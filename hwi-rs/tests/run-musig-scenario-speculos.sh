#!/usr/bin/env bash
# Scaffold for the MuSig2 speculos integration test.
#
# At this stage the script only sets up the moving parts that the
# subsequent commits will exercise:
#   * Boots speculos with the Ledger Bitcoin app.
#   * Boots a MuSig2/BIP388 capable bitcoind (Sjors's
#     2025/06/musig2-power branch) with -signer pointing at
#     hwi-rs through speculos-signer.sh.
#   * Creates a single Bitcoin Core wallet ("musig_hww") with
#     external_signer=true, blank=true, and private keys enabled.
#     addhdkey generates a hot HD key for cosigner B; derivehdkey with
#     private=true exports the xprv at m/87h/1h/0h. The
#     tr(musig(A_xpub, B_xprv)/<0;1>/*) descriptor is imported into
#     the same wallet, and a receive address is derived as a smoke
#     test.
#
# Subsequent commits hang the actual command-under-test (register,
# then policy-mode displayaddress) off this scaffold.
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
RPCPORT=28453
SIGNER="$REPO_ROOT/hwi-rs/tests/speculos-signer.sh"
APDU_PORT=9999
SPECULOS_API_PORT=5000

POLICY_NAME="MuSigTest"
POLICY_TEMPLATE='tr(musig(@0,@1)/**)'

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

start_autopress() {
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
}

stop_autopress() {
    if [[ -n "${AUTOPRESS_PID:-}" ]]; then
        kill "$AUTOPRESS_PID" 2>/dev/null || true
        wait "$AUTOPRESS_PID" 2>/dev/null || true
        AUTOPRESS_PID=""
    fi
}

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
FP_A="$(echo "$ENUM_RAW" | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 1, f"expected one device, got {entries!r}"
print(entries[0]["fingerprint"])
')"
echo "speculos master fingerprint: $FP_A"

echo "== fetching speculos xpub at m/87h/1h/0h via hwi-rs getxpub"
# BIP87 is outside the Ledger Bitcoin app's standard-path whitelist
# (BIP44/49/84/86 + BIP48-multisig), so the device prompts for
# confirmation. hwi-rs's getxpub always opts in to that prompt.
start_autopress
COSIGNER_A_XPUB="$(HWI_RS_LEDGER_SIMULATOR=1 "$HWI_RS_BIN" \
        --fingerprint "$FP_A" --chain test \
    getxpub "m/87'/1'/0'" | python3 -c 'import json,sys; print(json.load(sys.stdin)["xpub"])')"
stop_autopress
COSIGNER_A_KEY="[${FP_A}/87h/1h/0h]${COSIGNER_A_XPUB}"
echo "cosigner A: $COSIGNER_A_KEY"

echo "== launching bitcoind (regtest) with -signer=$SIGNER"
"$BITCOIND" -regtest -datadir="$DATADIR" -daemon \
    -signer="$SIGNER" \
    -fallbackfee=0.0001 \
    -rpcport="$RPCPORT" -port=28454 -listen=0

CLI=("$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT")

echo "== waiting for RPC"
for _ in $(seq 1 30); do
    if "${CLI[@]}" getblockchaininfo >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "== createwallet musig_hww (external_signer=true, blank, private keys enabled)"
# Single createwallet call:
#   - external_signer=true so the wallet is signer-aware from birth.
#     We still have to reload after importdescriptors (see below)
#     before registerpolicy works — Core only attaches the new
#     descriptor to an ExternalSignerScriptPubKeyMan on wallet load,
#     not on import — but at least we don't have to flip the flag.
#   - blank=true so Core skips the BIP44/49/84/86 device-descriptor
#     auto-import (Sjors's b990dbb504 "don't import external keys at
#     creation if blank"). Without that we'd end up with eight
#     unwanted xpub-only descriptors derived from the device, and
#     derivehdkey below would need an `hdkey=` disambiguator.
#   - disable_private_keys=false (override the external_signer default
#     of true) lets us addhdkey a hot HD seed for cosigner B and
#     import the watch-only musig(...) descriptor (which carries B's
#     xprv inline, so it counts as having private keys).
"${CLI[@]}" -named createwallet \
    wallet_name=musig_hww \
    descriptors=true \
    disable_private_keys=false \
    external_signer=true \
    blank=true >/dev/null

WCLI=("${CLI[@]}" -rpcwallet=musig_hww)

echo "== addhdkey: generate cosigner B's hot HD key inside musig_hww"
"${WCLI[@]}" addhdkey >/dev/null

echo "== derivehdkey for cosigner B at m/87h/1h/0h (private=true to get xprv)"
COSIGNER_B_KEY="$("${CLI[@]}" -rpcwallet=musig_hww -named derivehdkey \
        path="m/87h/1h/0h" private=true \
    | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["origin"] + v["xprv"])
')"
# Build a parallel xpub-only key for displayaddress (which doesn't
# need the secret).
COSIGNER_B_KEY_PUB="$("${CLI[@]}" -rpcwallet=musig_hww -named derivehdkey \
        path="m/87h/1h/0h" \
    | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["origin"] + v["xpub"])
')"
echo "cosigner B (xprv): $COSIGNER_B_KEY"

# Single multipath descriptor — Bitcoin Core expands tr(.../<0;1>/*)
# into matching receive (/0/*) and change (/1/*) script-pubkey
# managers on importdescriptors, which is what BIP388's DerivePolicy
# walks as a pair when registerpolicy fires.
DESC_NO_CKSUM="tr(musig(${COSIGNER_A_KEY},${COSIGNER_B_KEY})/<0;1>/*)"

echo "== adding checksum via getdescriptorinfo"
# `descriptor` in the response collapses to /0/* — use the top-level
# `checksum` field, which is the checksum of the original (multipath)
# descriptor as supplied.
CKSUM="$("${CLI[@]}" getdescriptorinfo "$DESC_NO_CKSUM" \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["checksum"])')"
DESC="${DESC_NO_CKSUM}#${CKSUM}"
echo "descriptor (B as xprv elided): tr(musig(${COSIGNER_A_KEY},${COSIGNER_B_KEY_PUB})/<0;1>/*)#${CKSUM}"

echo "== importdescriptors (musig multipath <0;1>)"
IMPORT_REQ="$(python3 -c "
import json, sys
print(json.dumps([{'desc': sys.argv[1], 'active': True, 'timestamp': 'now'}]))
" "$DESC")"
"${WCLI[@]}" importdescriptors "$IMPORT_REQ" \
    | python3 -c '
import json, sys
res = json.load(sys.stdin)
for r in res:
    assert r.get("success") is True, f"importdescriptors failed: {r!r}"
'

echo "== getnewaddress (bech32m, derived from imported musig descriptor)"
ADDR="$("${WCLI[@]}" getnewaddress "" bech32m)"
echo "receive address: $ADDR"
case "$ADDR" in
    bcrt1p*) ;;
    *) echo "unexpected address format (expected bcrt1p...): $ADDR" >&2; exit 1 ;;
esac

echo "== registerpolicy (Core -> hwi-rs register -> speculos), autoclicking"
start_autopress
REG_OUT="$("${WCLI[@]}" registerpolicy "$POLICY_NAME")"
stop_autopress
echo "$REG_OUT"
HMAC="$(echo "$REG_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["hmac"])')"
echo "registered hmac: $HMAC"

# Cross-check: the hmac just returned by registerpolicy must match
# what Core persists in the wallet's bip388[] table.
echo "== getwalletinfo bip388 entry"
WI="$("${WCLI[@]}" getwalletinfo)"
echo "$WI" | FP="$FP_A" HMAC="$HMAC" NAME="$POLICY_NAME" python3 -c '
import json, os, sys
w = json.load(sys.stdin)
hmacs = w.get("bip388", [])
assert hmacs, f"no bip388 hmacs in getwalletinfo: {w!r}"
match = next(
    (h for h in hmacs
     if h["name"] == os.environ["NAME"]
     and h["fingerprint"] == os.environ["FP"]),
    None,
)
assert match is not None, f"no matching bip388 entry in {hmacs!r}"
stored = match["hmac"]
expected = os.environ["HMAC"]
assert stored == expected, f"hmac mismatch: stored {stored} vs registerpolicy {expected}"
'

echo "== OK: registered MuSig2 wallet policy via hwi-rs register"

echo "== walletdisplayaddress (Core -> hwi-rs displayaddress -> speculos), autoclicking"
# walletdisplayaddress detects that the address belongs to a registered
# BIP388 policy and dispatches through ExternalSigner::DisplayAddressPolicy
# (which shells out to `hwi-rs displayaddress --policy-name ... --hmac ...`),
# so we don't have to assemble the policy template + keys + hmac here.
start_autopress
WDA_OUT="$("${WCLI[@]}" walletdisplayaddress "$ADDR")"
stop_autopress
echo "$WDA_OUT"
WDA_ADDR="$(echo "$WDA_OUT" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["address"])')"
# walletdisplayaddress echoes the input address on success; the
# device-vs-Core address comparison happens inside Core itself
# (ExternalSignerScriptPubKeyMan::DisplayAddressPolicy) and a mismatch
# would have produced an RPC error above.
[[ "$WDA_ADDR" == "$ADDR" ]] || { echo "walletdisplayaddress echoed unexpected address: $WDA_ADDR" >&2; exit 1; }

echo "== OK: drove on-device address display for the registered MuSig2 policy"
