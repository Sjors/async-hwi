#!/usr/bin/env bash
# MuSig2 / BIP388 happy-path scenario:
#
#   * Boot speculos with the Ledger Bitcoin app.
#   * Boot a MuSig2/BIP388 capable bitcoind (Sjors's
#     2025/06/musig2-power branch) with -signer pointing at
#     hwi-rs through speculos-signer.sh.
#   * Build the `musig_hww` wallet (cosigner A on the device,
#     cosigner B as a hot key) and import a tr(musig(...)) descriptor.
#   * Register the policy on the device, derive an address, drive
#     walletdisplayaddress on it.
#   * Fund the address from a helper miner wallet, then spend it
#     back through MuSig2 in a single `send` call (which runs both
#     rounds in-process and asserts complete=true), and verify the
#     spend confirms.
#
# The companion script `run-musig-disconnect-scenario-speculos.sh`
# exercises the device-offline flow using the same scaffold.
#
# Required env (all defaulted to the layout produced by the matching
# CI workflow, see .github/workflows/main.yml):
#   BITCOIND        Path to a MuSig2/BIP388 capable bitcoind. Default:
#                   ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI     Matching bitcoin-cli. Default:
#                   ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN      Path to hwi-rs binary. Default: ./target/release/hwi-rs
#   LEDGER_APP_ELF  Path to the Ledger Bitcoin app .elf. Required.
#   SPECULOS        Path to the speculos entry point. Default: speculos

set -Eeuo pipefail

# shellcheck source=lib-musig.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-musig.sh"

require_files
setup_datadir
trap cleanup_all EXIT

start_speculos

echo "== probing speculos via hwi-rs enumerate"
FP_A="$(get_speculos_fingerprint)"
echo "speculos master fingerprint: $FP_A"

echo "== fetching speculos xpub at m/87h/1h/0h via hwi-rs getxpub"
COSIGNER_A_XPUB="$(get_speculos_xpub "$FP_A" "m/87'/1'/0'")"
COSIGNER_A_KEY="[${FP_A}/87h/1h/0h]${COSIGNER_A_XPUB}"
echo "cosigner A: $COSIGNER_A_KEY"

start_bitcoind
setup_musig_wallet "$COSIGNER_A_KEY"

echo "== getnewaddress (bech32m, derived from imported musig descriptor)"
ADDR="$(wallet_cli getnewaddress "" bech32m)"
echo "receive address: $ADDR"
case "$ADDR" in
    bcrt1p*) ;;
    *) echo "unexpected address format (expected bcrt1p...): $ADDR" >&2; exit 1 ;;
esac

register_musig_policy "$FP_A"
echo "== OK: registered MuSig2 wallet policy via hwi-rs register"

echo "== walletdisplayaddress (Core -> hwi-rs displayaddress -> speculos), autoclicking"
# walletdisplayaddress detects that the address belongs to a registered
# BIP388 policy and dispatches through ExternalSigner::DisplayAddressPolicy
# (which shells out to `hwi-rs displayaddress --policy-name ... --hmac ...`),
# so we don't have to assemble the policy template + keys + hmac here.
start_autopress
WDA_OUT="$(wallet_cli walletdisplayaddress "$ADDR")"
stop_autopress
echo "$WDA_OUT"
WDA_ADDR="$(echo "$WDA_OUT" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["address"])')"
# walletdisplayaddress echoes the input address on success; the
# device-vs-Core address comparison happens inside Core itself
# (ExternalSignerScriptPubKeyMan::DisplayAddressPolicy) and a mismatch
# would have produced an RPC error above.
[[ "$WDA_ADDR" == "$ADDR" ]] || { echo "walletdisplayaddress echoed unexpected address: $WDA_ADDR" >&2; exit 1; }

echo "== OK: drove on-device address display for the registered MuSig2 policy"

# ---------------------------------------------------------------------
# Funding + MuSig2 spend round-trip.
#
# Prove that walletprocesspsbt routes through the new BIP388-aware
# FillPSBTPolicy path: round 1 produces both cosigners' MuSig2 pub
# nonces, round 2 produces both partial signatures, finalizepsbt
# aggregates them into a Schnorr key-path signature, and the resulting
# transaction is accepted by bitcoind regtest.
# ---------------------------------------------------------------------

setup_miner_wallet

echo "== funding $WALLET_NAME receive address $ADDR with 1.0 BTC"
FUND_TXID="$(miner_cli -named sendtoaddress address="$ADDR" amount=1.0)"
echo "fund txid: $FUND_TXID"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null

# Sanity: musig_hww should now see the UTXO.
BAL="$(wallet_cli getbalance)"
echo "$WALLET_NAME balance: $BAL"
python3 - <<PY
b = float("$BAL")
assert b >= 0.999, f"unexpected $WALLET_NAME balance: {b}"
PY

# Spend back to a fresh address owned by the miner wallet (cleanest
# regtest-valid destination).
DEST_ADDR="$(miner_cli getnewaddress "" bech32m)"

echo "== send (single call: expect both rounds to run, complete=true)"
# With the FinishTransaction prototype, `send` calls FillPSBT(sign=true)
# twice when the first pass leaves the PSBT incomplete. Round 1 produces
# nonces, round 2 produces partial sigs, FillPSBTPolicy aggregates with
# FinalizePSBT, and we get a complete tx in one shot. The Ledger
# autoclicker has to keep up with two confirmations.
start_autopress
SEND_OUT="$(wallet_cli -named send \
    outputs="[{\"$DEST_ADDR\": 0.5}]" \
    fee_rate=5)"
stop_autopress
COMPLETE="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
echo "send complete=$COMPLETE"
[[ "$COMPLETE" == "True" ]] || { echo "send did not complete in one call: $SEND_OUT" >&2; exit 1; }
SPEND_TXID="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["txid"])')"
echo "spend txid: $SPEND_TXID"

echo "== mine a confirmation block and verify the spend confirmed"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null
wallet_cli gettransaction "$SPEND_TXID" \
    | python3 -c 'import json,sys;t=json.load(sys.stdin);assert t["confirmations"] >= 1, t;print("confirmations:", t["confirmations"])'

echo "== OK: signed and broadcast a MuSig2 spend through hwi-rs"
