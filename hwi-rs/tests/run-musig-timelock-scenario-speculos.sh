#!/usr/bin/env bash
# MuSig2 / BIP388 with a tapleaf timelock fallback:
#
#   tr(
#     musig(@0, @1)/<0;1>/*,                          # key path:    A + B (MuSig2)
#     and_v(v:pk(@2/<0;1>/*), older(10))              # script path: A solo after 10 blocks
#   )
#
# where @0 is the Ledger (cosigner A) at m/87'/1'/0', @1 is a hot key
# (cosigner B) at m/87'/1'/0', and @2 is a second Ledger key at
# m/86'/1'/0' that can spend unilaterally once the input matures.
#
# Phase 1: in a hot wallet that holds B's xprv, register the policy,
# fund an address, and spend it via the MuSig2 key path (autopress
# walks the device through the standard two-round flow).
#
# Phase 2: in a brand-new watch-only Core wallet (no xprv for B,
# external_signer still pointed at speculos), import the same
# descriptor with B as an xpub and register the policy. Fund a fresh
# address from the miner.
#   * Build a PSBT with nSequence=10 on the input and run
#     walletprocesspsbt sign=true finalize=true: the MuSig2 key path
#     can't satisfy without B, so the device signs the tapleaf
#     branch and FinalizePSBT assembles a script-path witness.
#   * sendrawtransaction must fail with `non-BIP68-final` because
#     the input has only ~1 confirmation.
#   * Mine 9 more blocks (total 10) and rebroadcast the SAME hex:
#     it now confirms.
#
# The reused-hex check is the real regression test: the signature is
# committed over (sequence, prevout, script, ...), so accepting the
# same bytes after the timelock matures proves the device signed the
# tapleaf branch correctly the first time around.
#
# Required env: see lib-musig.sh / run-musig-scenario-speculos.sh.

set -Eeuo pipefail

# shellcheck source=lib-musig.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-musig.sh"

require_files
setup_datadir
trap cleanup_all EXIT

# Distinguish from the simpler scenarios in case multiple test runs
# leave registered policies on the same speculos NVRAM image.
POLICY_NAME="MuSigTimelock"
HOT_WALLET="musig_timelock_hot"
WATCH_WALLET="musig_timelock_watch"

start_speculos

FP_A="$(get_speculos_fingerprint)"
echo "speculos master fingerprint: $FP_A"

echo "== fetching speculos xpubs at m/87'/1'/0' (musig) and m/86'/1'/0' (solo)"
COSIGNER_A_MUSIG_XPUB="$(get_speculos_xpub "$FP_A" "m/87'/1'/0'")"
COSIGNER_A_SOLO_XPUB="$(get_speculos_xpub  "$FP_A" "m/86'/1'/0'")"
COSIGNER_A_MUSIG_KEY="[${FP_A}/87h/1h/0h]${COSIGNER_A_MUSIG_XPUB}"
COSIGNER_A_SOLO_KEY="[${FP_A}/86h/1h/0h]${COSIGNER_A_SOLO_XPUB}"
echo "cosigner A (musig): $COSIGNER_A_MUSIG_KEY"
echo "cosigner A (solo):  $COSIGNER_A_SOLO_KEY"

start_bitcoind

# ---------------------------------------------------------------------
# Phase 1: hot wallet, normal MuSig2 key-path spend.
# ---------------------------------------------------------------------

WALLET_NAME="$HOT_WALLET"
create_signer_wallet_with_hot_b "$HOT_WALLET"

# Single descriptor template (B in xpub-only form). The hot wallet binds
# B's xprv automatically at importdescriptors time via the master
# fingerprint; the watch-only wallet just stores the xpub.
DESC_TEMPLATE="tr(musig(${COSIGNER_A_MUSIG_KEY},${COSIGNER_B_KEY})/<0;1>/*,and_v(v:pk(${COSIGNER_A_SOLO_KEY}/<0;1>/*),older(10)))"
echo "descriptor: $DESC_TEMPLATE"

import_active_descriptor "$DESC_TEMPLATE" "$HOT_WALLET"

ADDR="$(wallet_cli_for "$HOT_WALLET" getnewaddress "" bech32m)"
echo "hot wallet receive address: $ADDR"
case "$ADDR" in
    bcrt1p*) ;;
    *) echo "unexpected address format (expected bcrt1p...): $ADDR" >&2; exit 1 ;;
esac

register_musig_policy "$FP_A" "$HOT_WALLET" "$POLICY_NAME"

setup_miner_wallet

echo "== funding $HOT_WALLET receive address $ADDR with 1.0 BTC"
miner_cli -named sendtoaddress address="$ADDR" amount=1.0 >/dev/null
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null

# Sanity: hot wallet should now see the UTXO.
HOT_BAL="$(wallet_cli_for "$HOT_WALLET" getbalance)"
python3 - <<PY
b = float("$HOT_BAL")
assert b >= 0.999, f"unexpected $HOT_WALLET balance: {b}"
PY

DEST_ADDR_1="$(miner_cli getnewaddress "" bech32m)"

echo "== send (MuSig2 key path, both signers, expect complete=true)"
# `send` on the hot wallet runs both MuSig2 rounds in one call: round
# 1 collects pub nonces from B (local) and A (device), round 2
# collects partial sigs, FillPSBTPolicy aggregates with FinalizePSBT.
# Two device confirmations, autopress takes care of them.
start_autopress
SEND_OUT="$(wallet_cli_for "$HOT_WALLET" -named send \
    outputs="[{\"$DEST_ADDR_1\": 0.5}]" \
    fee_rate=5)"
stop_autopress
COMPLETE="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
[[ "$COMPLETE" == "True" ]] || { echo "key-path send did not complete: $SEND_OUT" >&2; exit 1; }
SPEND_TXID="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["txid"])')"
echo "key-path spend txid: $SPEND_TXID"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null
wallet_cli_for "$HOT_WALLET" gettransaction "$SPEND_TXID" \
    | python3 -c 'import json,sys;t=json.load(sys.stdin);assert t["confirmations"] >= 1, t;print("key-path confirmations:", t["confirmations"])'

echo "== OK: phase 1 (MuSig2 key-path spend with timelock-capable descriptor)"

# ---------------------------------------------------------------------
# Phase 2: watch-only wallet, tapleaf timelock spend.
# ---------------------------------------------------------------------

WALLET_NAME="$WATCH_WALLET"
create_signer_watchonly_wallet "$WATCH_WALLET"
import_active_descriptor "$DESC_TEMPLATE" "$WATCH_WALLET"

WATCH_ADDR="$(wallet_cli_for "$WATCH_WALLET" getnewaddress "" bech32m)"
echo "watch-only wallet receive address: $WATCH_ADDR"

register_musig_policy "$FP_A" "$WATCH_WALLET" "$POLICY_NAME"

echo "== funding $WATCH_WALLET receive address $WATCH_ADDR with 1.0 BTC"
FUND_TXID="$(miner_cli -named sendtoaddress address="$WATCH_ADDR" amount=1.0)"
echo "fund txid: $FUND_TXID"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null

# Locate the funding UTXO from the watch wallet's PoV.
read -r FUND_VOUT FUND_AMOUNT < <(wallet_cli_for "$WATCH_WALLET" listunspent 1 9999 "[\"$WATCH_ADDR\"]" \
    | TXID="$FUND_TXID" python3 -c '
import json, os, sys
want = os.environ["TXID"]
utxos = [u for u in json.load(sys.stdin) if u["txid"] == want]
assert len(utxos) == 1, f"expected one UTXO for {want!r}, got {utxos!r}"
u = utxos[0]
print(u["vout"], u["amount"])
')
echo "funding UTXO: $FUND_TXID:$FUND_VOUT ($FUND_AMOUNT BTC)"

DEST_ADDR_2="$(miner_cli getnewaddress "" bech32m)"

echo "== walletcreatefundedpsbt with explicit nSequence=10 on the input"
# Setting sequence=10 on the input arms BIP68 / OP_CSV: the tapleaf's
# `older(10)` branch becomes satisfiable once the input has 10
# confirmations. Without an explicit sequence here the wallet would
# default to the BIP125 RBF sequence (0xfffffffd) which clears the
# OP_CSV-relevant bits.
INPUTS_JSON="$(TXID="$FUND_TXID" VOUT="$FUND_VOUT" python3 -c '
import json, os
print(json.dumps([{"txid": os.environ["TXID"], "vout": int(os.environ["VOUT"]), "sequence": 10}]))
')"
PSBT_RAW="$(wallet_cli_for "$WATCH_WALLET" -named walletcreatefundedpsbt \
    inputs="$INPUTS_JSON" \
    outputs="[{\"$DEST_ADDR_2\": 0.5}]" \
    options='{"fee_rate": 5, "add_inputs": false}' \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["psbt"])')"
echo "unsigned PSBT length: ${#PSBT_RAW}"

echo "== walletprocesspsbt sign=true finalize=true (tapleaf path on Ledger), autoclicking"
# The MuSig2 key path can't satisfy because the watch-only wallet has
# no privkey for B. The Ledger sees the registered policy + an input
# whose sequence makes the older(10) branch viable, signs the
# tapleaf, and Core's FinalizePSBT assembles a [sig, script, control]
# witness.
start_autopress
WPP_OUT="$(wallet_cli_for "$WATCH_WALLET" -named walletprocesspsbt \
    psbt="$PSBT_RAW" \
    sign=true \
    finalize=true)"
stop_autopress

COMPLETE_TL="$(echo "$WPP_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
[[ "$COMPLETE_TL" == "True" ]] || { echo "tapleaf walletprocesspsbt did not complete: $WPP_OUT" >&2; exit 1; }
SPEND_HEX="$(echo "$WPP_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["hex"])')"
[[ -n "$SPEND_HEX" ]] || { echo "walletprocesspsbt complete=true but no hex returned" >&2; exit 1; }

# Decode the tx to assert the witness is the script-path shape we
# expect (3 elements: tapleaf sig, script, control block) rather than
# the key-path shape (1 element: a single Schnorr sig). This catches
# the device accidentally signing the wrong branch.
echo "== verify witness is script-path (3 elements)"
core_cli decoderawtransaction "$SPEND_HEX" | python3 -c '
import json, sys
tx = json.load(sys.stdin)
vins = tx.get("vin", [])
assert len(vins) == 1, f"expected 1 input, got {vins!r}"
wit = vins[0].get("txinwitness", [])
assert len(wit) == 3, f"expected 3 witness elements (script-path), got {len(wit)}: {wit!r}"
seq = vins[0]["sequence"]
assert seq == 10, f"expected nSequence=10, got {seq}"
print(f"witness elements: {len(wit)}, sequence: {seq}")
'

echo "== sendrawtransaction (must fail: input only has ~1 confirmation)"
# BIP68 enforces relative timelocks at mempool admission. The tx is
# script-valid in isolation but `non-BIP68-final` until the input
# matures, so bitcoind must reject it.
if BCAST_OUT="$(core_cli sendrawtransaction "$SPEND_HEX" 2>&1)"; then
    echo "expected sendrawtransaction to fail before maturity, got: $BCAST_OUT" >&2
    exit 1
fi
echo "$BCAST_OUT" | grep -q -i 'non-bip68-final\|non-final\|bad-txns-nonfinal' \
    || { echo "unexpected error from premature broadcast: $BCAST_OUT" >&2; exit 1; }
echo "got expected non-BIP68-final rejection: $(echo "$BCAST_OUT" | tr '\n' ' ' | head -c 200)"

echo "== mine 9 more blocks so the input has 10 confirmations"
miner_cli generatetoaddress 9 "$MINER_ADDR" >/dev/null

echo "== rebroadcast the SAME hex (the timelock has now matured)"
# Reusing the same hex bytes is the actual regression test: signatures
# commit to the input's sequence + script + prevout, so a fresh
# acceptance after maturity proves the original Ledger signature is
# valid for the tapleaf branch.
SPEND_TXID_TL="$(core_cli sendrawtransaction "$SPEND_HEX")"
echo "tapleaf spend txid: $SPEND_TXID_TL"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null
wallet_cli_for "$WATCH_WALLET" gettransaction "$SPEND_TXID_TL" \
    | python3 -c 'import json,sys;t=json.load(sys.stdin);assert t["confirmations"] >= 1, t;print("tapleaf confirmations:", t["confirmations"])'

echo "== OK: signed and broadcast a tapleaf timelock spend through hwi-rs"
