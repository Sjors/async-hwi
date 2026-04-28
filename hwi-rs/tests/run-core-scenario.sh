#!/usr/bin/env bash
# End-to-end test: drive Bitcoin Core's external-signer interface against
# `hwi-rs` running in mock mode. Used by CI and runnable locally.
#
# Required env (set automatically in CI):
#   BITCOIND       Path to bitcoind. Default: ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI    Path to bitcoin-cli. Default: ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN     Path to the hwi-rs binary. Default: ./target/release/hwi-rs

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BITCOIND="${BITCOIND:-$REPO_ROOT/bitcoin-core/build/bin/bitcoind}"
BITCOIN_CLI="${BITCOIN_CLI:-$REPO_ROOT/bitcoin-core/build/bin/bitcoin-cli}"
export HWI_RS_BIN="${HWI_RS_BIN:-$REPO_ROOT/target/release/hwi-rs}"

for f in "$BITCOIND" "$BITCOIN_CLI" "$HWI_RS_BIN"; do
    if [[ ! -x "$f" ]]; then
        echo "missing executable: $f" >&2
        exit 1
    fi
done

DATADIR="$(mktemp -d)"
RPCPORT=28443
SIGNER="$REPO_ROOT/hwi-rs/tests/mock-signer.sh"

cleanup() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" stop >/dev/null 2>&1 || true
    sleep 1
    rm -rf "$DATADIR"
}
trap cleanup EXIT

echo "== launching bitcoind (regtest) with -signer=$SIGNER"
"$BITCOIND" -regtest -datadir="$DATADIR" -daemon \
    -signer="$SIGNER" \
    -fallbackfee=0.0001 \
    -rpcport="$RPCPORT" -port=28444 -listen=0

CLI=("$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT")

echo "== waiting for RPC"
for _ in $(seq 1 30); do
    if "${CLI[@]}" getblockchaininfo >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "== enumeratesigners"
ENUM_OUT="$("${CLI[@]}" enumeratesigners)"
echo "$ENUM_OUT"
echo "$ENUM_OUT" | python3 -c '
import json, sys
data = json.load(sys.stdin)
signers = data.get("signers", [])
assert len(signers) == 1, f"expected exactly one signer, got {signers!r}"
# Master fingerprint of BIP32 test vector 1, baked into the mock device.
assert signers[0].get("fingerprint") == "3442193e", signers
'

echo "== createwallet (external_signer=true, regtest)"
# Not blank: as of Sjors's 2025/07/external-signer-relax (commit
# b990dbb504 "wallet: don't import external keys at creation if
# blank"), `blank=true` skips fetching descriptors from the external
# signer entirely, and `getnewaddress` then fails with "no available
# keys".
"${CLI[@]}" -named createwallet \
    wallet_name=hww \
    disable_private_keys=true \
    descriptors=true \
    external_signer=true

echo "== getwalletinfo"
WI="$("${CLI[@]}" -rpcwallet=hww getwalletinfo)"
echo "$WI" | python3 -c '
import json, sys
w = json.load(sys.stdin)
assert w.get("external_signer") is True, f"external_signer flag not set: {w!r}"
'

echo "== getnewaddress (must derive locally from imported descriptors)"
ADDR="$("${CLI[@]}" -rpcwallet=hww getnewaddress)"
echo "$ADDR"
case "$ADDR" in
    bcrt1*) ;;  # bech32 regtest address
    *) echo "unexpected address format: $ADDR" >&2; exit 1 ;;
esac

echo "== walletdisplayaddress (echoes back via signer)"
DISP="$("${CLI[@]}" -rpcwallet=hww walletdisplayaddress "$ADDR")"
echo "$DISP"
echo "$DISP" | python3 -c "
import json, sys
got = json.load(sys.stdin).get('address')
want = '$ADDR'
assert got == want, f'walletdisplayaddress returned {got!r}, expected {want!r}'
"

echo "== signtx via --stdin (empty PSBT round-trip)"
EMPTY_PSBT="cHNidP8BAAoCAAAAAAAAAAAAAA=="
SIGN_OUT="$(printf 'signtx %s\n' "$EMPTY_PSBT" \
    | HWI_RS_MOCK=1 "$HWI_RS_BIN" --stdin --fingerprint 3442193e --chain regtest)"
echo "$SIGN_OUT"
echo "$SIGN_OUT" | python3 -c "
import json, sys
out = json.load(sys.stdin)
assert out.get('psbt') == '$EMPTY_PSBT', out
"

echo "== fund wallet and sign a real PSBT through walletprocesspsbt"
"${CLI[@]}" generatetoaddress 101 "$ADDR" >/dev/null
BURN="$("${CLI[@]}" -rpcwallet=hww getnewaddress)"
FUND="$("${CLI[@]}" -rpcwallet=hww -named walletcreatefundedpsbt \
    outputs="[{\"$BURN\":1.0}]" \
    options='{"feeRate":0.00010000}')"
PSBT="$(echo "$FUND" | python3 -c 'import json,sys;print(json.load(sys.stdin)["psbt"])')"
SIGNED="$("${CLI[@]}" -rpcwallet=hww walletprocesspsbt "$PSBT" true)"
echo "$SIGNED" | python3 -c '
import base64, json, sys
out = json.load(sys.stdin)
assert out.get("complete") is True, f"PSBT not fully signed: {out!r}"
'

echo "== OK"
