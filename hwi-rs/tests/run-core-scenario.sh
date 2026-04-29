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

echo "== OK"
