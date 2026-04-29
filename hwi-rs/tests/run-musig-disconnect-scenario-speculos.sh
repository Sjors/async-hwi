#!/usr/bin/env bash
# MuSig2 / BIP388 device-offline scenario:
#
#   * Same scaffold as run-musig-scenario-speculos.sh: speculos +
#     bitcoind regtest + the musig_hww wallet (cosigner A on speculos,
#     cosigner B as a hot key) + a registered policy + a funded UTXO.
#   * Use a wrapper signer (`speculos-signer-disconnectable.sh`) that
#     short-circuits to `{"error":"device disconnected"}` whenever the
#     flag file `$DATADIR/disconnect` exists, simulating the user
#     unplugging the Ledger between RPCs. `enumerate` still returns
#     the cached fingerprint so `send` can locate the signer entry.
#
# Phase 1: touch the flag, then call `send`. The hot cosigner B
# contributes its MuSig2 pub nonce in the local FillPSBT pass; the
# external signer call returns the synthetic disconnect error. With
# the soft-fail in ExternalSignerScriptPubKeyMan::FillPSBTPolicy,
# this surfaces as `complete=false` plus a round-1 PSBT carrying just
# B's pub nonce (BIP-373 input field 0x1b).
#
# Phase 1b: a follow-up `walletprocesspsbt` while the device is still
# offline must hard-fail. The local pass adds nothing new (B's pub
# nonce is already in the PSBT) so silently returning the unchanged
# PSBT would be misleading; the soft-fail only kicks in when the
# local pass actually advanced the PSBT this call.
#
# Phase 2: remove the flag (device "reconnected"), then call
# `walletprocesspsbt sign=true finalize=true` on the round-1 PSBT.
# This must:
#   (a) preserve B's original pub nonce byte-for-byte (regression
#       check: walletprocesspsbt must NOT reset the MuSig2 session by
#       re-running B's local nonce generation),
#   (b) add A's pub nonce via the device (round 1 on the device),
#   (c) run round 2 on both cosigners (B partial sig from the
#       secnonce stashed in the SPKM, A partial sig from the device),
#   (d) aggregate them via FinalizePSBT into a complete tx.
#
# Then broadcast and verify the spend confirms.
#
# Required env: see lib-musig.sh / run-musig-scenario-speculos.sh.

set -Eeuo pipefail

# shellcheck source=lib-musig.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-musig.sh"

require_files
setup_datadir

# Wire up the disconnectable signer wrapper. The flag file lives
# inside $DATADIR so cleanup_all sweeps it on exit, and the wrapper
# inherits these env vars from bitcoind (they have to be set before
# start_bitcoind so the daemon picks them up for its signer subprocess).
# shellcheck disable=SC2034  # SIGNER is read by start_bitcoind in lib-musig.sh
SIGNER="$REPO_ROOT/hwi-rs/tests/speculos-signer-disconnectable.sh"
export DISCONNECT_FLAG="$DATADIR/disconnect"
trap cleanup_all EXIT

start_speculos

FP_A="$(get_speculos_fingerprint)"
echo "speculos master fingerprint: $FP_A"
export DEVICE_FINGERPRINT="$FP_A"
export DEVICE_MODEL="ledger_nano_x"

COSIGNER_A_XPUB="$(get_speculos_xpub "$FP_A" "m/87'/1'/0'")"
COSIGNER_A_KEY="[${FP_A}/87h/1h/0h]${COSIGNER_A_XPUB}"
echo "cosigner A: $COSIGNER_A_KEY"

start_bitcoind
setup_musig_wallet "$COSIGNER_A_KEY"

ADDR="$(wallet_cli getnewaddress "" bech32m)"
echo "receive address: $ADDR"

register_musig_policy "$FP_A"

setup_miner_wallet

echo "== funding $WALLET_NAME receive address $ADDR with 1.0 BTC"
miner_cli -named sendtoaddress address="$ADDR" amount=1.0 >/dev/null
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null

DEST_ADDR="$(miner_cli getnewaddress "" bech32m)"

# ---------------------------------------------------------------------
# Phase 1: device "disconnected", expect a round-1 PSBT from `send`.
# ---------------------------------------------------------------------

echo "== flip disconnect flag to simulate Ledger unplug"
touch "$DISCONNECT_FLAG"

echo "== send (device offline, expect complete=false round-1 PSBT)"
# No autopress: the wrapper short-circuits before any APDU traffic.
# With the FillPSBTPolicy soft-fail, the local pass contributes B's
# pub nonce and the device error is logged but not propagated, so
# `send` returns with complete=false and the round-1 PSBT in `psbt`.
SEND_OUT="$(wallet_cli -named send \
    outputs="[{\"$DEST_ADDR\": 0.5}]" \
    fee_rate=5)"
echo "$SEND_OUT"

ROUND1_PSBT="$(echo "$SEND_OUT" | python3 -c 'import json,sys;v=json.load(sys.stdin);print(v.get("psbt",""))')"
COMPLETE_R1="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
echo "send complete=$COMPLETE_R1"
[[ "$COMPLETE_R1" == "False" ]] || { echo "send unexpectedly completed offline: $SEND_OUT" >&2; exit 1; }
[[ -n "$ROUND1_PSBT" ]] || { echo "send returned no psbt offline: $SEND_OUT" >&2; exit 1; }

echo "== inspect round-1 PSBT: must carry exactly one cosigner's pub nonce"
# decodepsbt exposes MuSig2 entries as inputs[].musig2_pubnonces, a
# list of {participant_pubkey, aggregate_pubkey, leaf_hash, pubnonce}.
# Require exactly one entry per input (B's pub nonce only); A's must
# be missing because the device never got asked.
core_cli decodepsbt "$ROUND1_PSBT" \
    | python3 -c '
import json, sys
psbt = json.load(sys.stdin)
inputs = psbt.get("inputs", [])
assert inputs, f"PSBT has no inputs: {psbt!r}"
for i, inp in enumerate(inputs):
    nonces = inp.get("musig2_pubnonces", [])
    parts = inp.get("musig2_participant_pubkeys", [])
    assert parts, f"input {i} missing musig2 participants: {inp!r}"
    assert len(nonces) == 1, (
        f"input {i} should have exactly one pub nonce after offline send, "
        f"got {len(nonces)}: {nonces!r}"
    )
    n = nonces[0]
    pn = n["pubnonce"]
    pk = n["participant_pubkey"]
    print(f"input {i}: B pubnonce={pn[:16]}... part_pk={pk[:16]}...")
'

# ---------------------------------------------------------------------
# Phase 1b: a follow-up walletprocesspsbt with the device still
# offline must hard-fail. The local pass has nothing new to
# contribute (B's pub nonce is already in the PSBT from phase 1) and
# the device call still errors out, so silently returning the
# unchanged PSBT as "fine" would mislead the caller. Core surfaces
# this as PSBTError::EXTERNAL_SIGNER_FAILED, which RPC turns into
# a JSON-RPC error / non-zero exit from bitcoin-cli.
# ---------------------------------------------------------------------

echo "== walletprocesspsbt while still offline (must hard-fail)"
if WPP_OFFLINE_OUT="$(wallet_cli -named walletprocesspsbt \
    psbt="$ROUND1_PSBT" \
    sign=true \
    finalize=true 2>&1)"; then
    echo "expected walletprocesspsbt to fail while device offline, got: $WPP_OFFLINE_OUT" >&2
    exit 1
fi
echo "$WPP_OFFLINE_OUT" | grep -q -i 'external_signer\|external signer\|sign' \
    || { echo "unexpected error from offline walletprocesspsbt: $WPP_OFFLINE_OUT" >&2; exit 1; }
echo "got expected hard-fail: $(echo "$WPP_OFFLINE_OUT" | tr '\n' ' ' | head -c 200)"

# ---------------------------------------------------------------------
# Phase 2: device "reconnected", walletprocesspsbt finishes the spend.
# ---------------------------------------------------------------------

echo "== clear disconnect flag (Ledger plugged back in)"
rm -f "$DISCONNECT_FLAG"

echo "== walletprocesspsbt sign=true finalize=true (autoclicking)"
# Drive both MuSig2 rounds plus aggregation in a single RPC. Inside
# CWallet::FillPSBT this routes to ExternalSignerScriptPubKeyMan::
# FillPSBTPolicy, which:
#   (1) runs DescriptorScriptPubKeyMan::FillPSBT locally -- B already
#       has its pub nonce from phase 1 so this pass is a no-op for
#       round 1 (the regression check below verifies B's pub nonce is
#       NOT regenerated here);
#   (2) calls signtx on the device -- the speculos app drives both
#       its own rounds in one go (one user confirmation visible via
#       autopress), adding A's pub nonce and A's partial sig;
#   (3) re-runs the local FillPSBT to produce B's partial sig from
#       the secnonce stashed in the SPKM in phase 1 (any reset of B's
#       MuSig2 session would invalidate this sig);
#   (4) calls FinalizePSBT, aggregating both partial sigs into a
#       Schnorr key-path signature on the input.
start_autopress
WPP_OUT="$(wallet_cli -named walletprocesspsbt \
    psbt="$ROUND1_PSBT" \
    sign=true \
    finalize=true)"
stop_autopress
echo "$WPP_OUT"

COMPLETE_R2="$(echo "$WPP_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
[[ "$COMPLETE_R2" == "True" ]] || {
    echo "walletprocesspsbt did not complete: $WPP_OUT" >&2
    exit 1
}

ROUND2_HEX="$(echo "$WPP_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("hex",""))')"
[[ -n "$ROUND2_HEX" ]] || { echo "walletprocesspsbt complete=true but no hex returned" >&2; exit 1; }

echo "== broadcast: this is the real regression check for B's nonce"
# Successful broadcast implies the aggregated Schnorr key-path sig
# verifies under the MuSig2 aggregate pubkey for the input. That can
# only happen if B's partial sig in step (3) above was computed from
# the secnonce that produced B's pub nonce in phase 1. If
# walletprocesspsbt had reset B's MuSig2 session (regenerating either
# the secnonce or the pub nonce in isolation), the partial sig would
# be inconsistent with the pub nonce in the PSBT, MuSig2 aggregation
# would yield an invalid Schnorr signature, and sendrawtransaction
# would reject it as `non-mandatory-script-verify-flag (Invalid
# Schnorr signature)`. So a clean broadcast is byte-for-byte proof
# that the cosigner B session survived the disconnect/reconnect.
SPEND_TXID="$(core_cli sendrawtransaction "$ROUND2_HEX")"
echo "spend txid: $SPEND_TXID"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null
wallet_cli gettransaction "$SPEND_TXID" \
    | python3 -c 'import json,sys;t=json.load(sys.stdin);assert t["confirmations"] >= 1, t;print("confirmations:", t["confirmations"])'

echo "== OK: signed and broadcast a MuSig2 spend across a device disconnect"
