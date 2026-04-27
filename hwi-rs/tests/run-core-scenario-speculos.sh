#!/usr/bin/env bash
# End-to-end test: drive Bitcoin Core's external-signer interface against
# `hwi-rs` running against a Speculos-emulated Ledger Nano X with the real
# Bitcoin app. Counterpart to run-core-scenario.sh, which uses the
# in-process software mock.
#
# Required env (set automatically in CI):
#   BITCOIND        Path to bitcoind. Default: ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI     Path to bitcoin-cli. Default: ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN      Path to hwi-rs binary. Default: ./target/release/hwi-rs
#   LEDGER_APP_ELF  Path to the Ledger Bitcoin app .elf. Required.
#   SPECULOS        Path to the speculos entry point. Default: speculos
#                   (must be on PATH, e.g. installed via `pip install speculos`)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BITCOIND="${BITCOIND:-$REPO_ROOT/bitcoin-core/build/bin/bitcoind}"
BITCOIN_CLI="${BITCOIN_CLI:-$REPO_ROOT/bitcoin-core/build/bin/bitcoin-cli}"
export HWI_RS_BIN="${HWI_RS_BIN:-$REPO_ROOT/target/release/hwi-rs}"
SPECULOS="${SPECULOS:-speculos}"
LEDGER_APP_ELF="${LEDGER_APP_ELF:?LEDGER_APP_ELF must point to the Ledger Bitcoin app .elf}"

for f in "$BITCOIND" "$BITCOIN_CLI" "$HWI_RS_BIN" "$LEDGER_APP_ELF"; do
    if [[ ! -e "$f" ]]; then
        echo "missing file: $f" >&2
        exit 1
    fi
done

DATADIR="$(mktemp -d)"
SPECULOS_LOG="$DATADIR/speculos.log"
AUTOPRESS_LOG="$DATADIR/autopress.log"
RPCPORT=28443
SIGNER="$REPO_ROOT/hwi-rs/tests/speculos-signer.sh"
APDU_PORT=9999
SPECULOS_API_PORT=5000

cleanup() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" stop >/dev/null 2>&1 || true
    if [[ -n "${AUTOPRESS_PID:-}" ]]; then
        kill "$AUTOPRESS_PID" 2>/dev/null || true
        wait "$AUTOPRESS_PID" 2>/dev/null || true
    fi
    if [[ -n "${SPECULOS_PID:-}" ]]; then
        kill "$SPECULOS_PID" 2>/dev/null || true
        wait "$SPECULOS_PID" 2>/dev/null || true
    fi
    sleep 1
    rm -rf "$DATADIR"
}
trap cleanup EXIT

echo "== launching speculos with $LEDGER_APP_ELF"
"$SPECULOS" \
    --model nanox \
    --display headless \
    --apdu-port "$APDU_PORT" \
    --api-port "$SPECULOS_API_PORT" \
    "$LEDGER_APP_ELF" \
    >"$SPECULOS_LOG" 2>&1 &
SPECULOS_PID=$!

echo "== waiting for speculos APDU port"
for _ in $(seq 1 60); do
    if (echo > "/dev/tcp/127.0.0.1/$APDU_PORT") 2>/dev/null; then
        break
    fi
    sleep 1
done
if ! (echo > "/dev/tcp/127.0.0.1/$APDU_PORT") 2>/dev/null; then
    echo "speculos failed to come up; log:" >&2
    cat "$SPECULOS_LOG" >&2 || true
    exit 1
fi

echo "== probing speculos via hwi-rs enumerate"
ENUM_RAW="$(HWI_RS_LEDGER_SIMULATOR=1 "$HWI_RS_BIN" enumerate)"
echo "$ENUM_RAW"
FP="$(echo "$ENUM_RAW" | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 1, f"expected one device, got {entries!r}"
e = entries[0]
assert e.get("error") in (None, ""), f"speculos error: {e!r}"
fp = e.get("fingerprint")
assert fp, f"no fingerprint reported: {e!r}"
print(fp)
')"
echo "speculos master fingerprint: $FP"

echo "== probing speculos via hwi-rs getdescriptors (regtest, account 0)"
DESC_RAW="$(HWI_RS_LEDGER_SIMULATOR=1 "$HWI_RS_BIN" --fingerprint "$FP" --chain regtest getdescriptors --account 0)"
echo "$DESC_RAW"
echo "$DESC_RAW" | FP="$FP" python3 -c "
import json, os, sys
out = json.load(sys.stdin)
fp = os.environ['FP']
recv = out.get('receive', [])
intl = out.get('internal', [])
assert len(recv) == 4 and len(intl) == 4, f'expected 4 receive + 4 internal, got {len(recv)} + {len(intl)}'
for d in recv + intl:
    assert '#' in d, f'descriptor missing checksum: {d}'
    assert fp in d, f'descriptor missing fingerprint {fp}: {d}'
"

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

echo "== enumeratesigners (Core's view)"
ENUM_OUT="$("${CLI[@]}" enumeratesigners)"
echo "$ENUM_OUT"
FP_CORE="$(echo "$ENUM_OUT" | python3 -c '
import json, sys
data = json.load(sys.stdin)
signers = data.get("signers", [])
assert len(signers) == 1, f"expected exactly one signer, got {signers!r}"
print(signers[0]["fingerprint"])
')"
if [[ "$FP_CORE" != "$FP" ]]; then
    echo "fingerprint mismatch: enumerate=$FP, enumeratesigners=$FP_CORE" >&2
    exit 1
fi

echo "== createwallet (external_signer=true, regtest)"
"${CLI[@]}" -named createwallet \
    wallet_name=hww \
    disable_private_keys=true \
    blank=true \
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
    bcrt1*) ;;  # bech32 / bech32m regtest
    *) echo "unexpected address format: $ADDR" >&2; exit 1 ;;
esac

echo "== fund the wallet so we have a UTXO to sign"
"${CLI[@]}" generatetoaddress 101 "$ADDR" >/dev/null
BURN="$("${CLI[@]}" -rpcwallet=hww getnewaddress)"
FUND="$("${CLI[@]}" -rpcwallet=hww -named walletcreatefundedpsbt \
    outputs="[{\"$BURN\":1.0}]" \
    options='{"feeRate":0.00010000}')"
PSBT="$(echo "$FUND" | python3 -c 'import json,sys;print(json.load(sys.stdin)["psbt"])')"
echo "PSBT to sign: $PSBT"

# Speculos's HTTP automation API exposes /button/{left,right,both} for
# directional and confirm presses. The Bitcoin app prompts at several
# steps (review, output amount, fee, confirm). We can't know the exact
# screen sequence ahead of time, so spawn a background loop that just
# spam-clicks the "right" (next/approve) button at a steady cadence
# until walletprocesspsbt returns. This is the same trick HWI's CI uses.
echo "== spawning autopress loop against speculos HTTP API"
(
    while true; do
        curl -fsS -X POST "http://127.0.0.1:$SPECULOS_API_PORT/button/right" \
            -H 'Content-Type: application/json' \
            -d '{"action":"press-and-release"}' >/dev/null 2>&1 || true
        curl -fsS -X POST "http://127.0.0.1:$SPECULOS_API_PORT/button/both" \
            -H 'Content-Type: application/json' \
            -d '{"action":"press-and-release"}' >/dev/null 2>&1 || true
        sleep 0.4
    done
) >"$AUTOPRESS_LOG" 2>&1 &
AUTOPRESS_PID=$!

echo "== walletprocesspsbt (drives Core -> hwi-rs --stdin signtx -> speculos)"
SIGNED="$("${CLI[@]}" -rpcwallet=hww walletprocesspsbt "$PSBT" true)"
echo "$SIGNED"

# Stop the autopress loop now that signing is done.
kill "$AUTOPRESS_PID" 2>/dev/null || true
wait "$AUTOPRESS_PID" 2>/dev/null || true
AUTOPRESS_PID=""

echo "$SIGNED" | python3 -c '
import json, sys
out = json.load(sys.stdin)
assert out.get("complete") is True, f"PSBT not fully signed by device: {out!r}"
'

echo "== OK"
