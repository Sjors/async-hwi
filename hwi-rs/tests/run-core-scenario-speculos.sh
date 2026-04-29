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
APDU_PORT=9999

cleanup() {
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
echo "$ENUM_RAW" | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 1, f"expected one device, got {entries!r}"
e = entries[0]
assert e.get("error") in (None, ""), f"speculos error: {e!r}"
assert e.get("fingerprint"), f"no fingerprint reported: {e!r}"
'

echo "== OK"
