#!/usr/bin/env bash
# MuSig2 / BIP388 happy-path scenario, Coldcard edition.
#
#   * Boot the `coldcard-mpy` simulator (natively or in podman).
#   * Boot a MuSig2/BIP388 capable bitcoind (Sjors's
#     2025/06/musig2-power branch) with -signer pointing at hwi-rs
#     through coldcard-signer.sh.
#   * Build the `musig_hww` wallet (cosigner A on the Coldcard, cosigner
#     B as a hot key) and import the tr(musig(A,B)/<0;1>/*) descriptor.
#   * Register the policy on the device and verify Bitcoin Core stores
#     the returned placeholder hmac.
#
# Coldcard differences vs the Ledger speculos scenario:
#   * BIP388 wallets on Coldcard are keyed by name, not by HMAC, so the
#     hmac round-tripped through Bitcoin Core's `getwalletinfo.bip388`
#     is a 32-byte zero placeholder produced by hwi-rs (see
#     COLDCARD_PLACEHOLDER_HMAC in src/devices/coldcard.rs).
#   * On-device confirmations are driven by `XKEY` keypress injection
#     embedded in hwi-rs (see coldcard-vendored sim_keypress), so
#     unlike the Ledger scenario there is no external autopressing
#     loop.
#
# Required env (set automatically in CI):
#   BITCOIND        Path to a MuSig2/BIP388 capable bitcoind. Default:
#                   ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI     Matching bitcoin-cli. Default:
#                   ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN      Path to hwi-rs binary. Default: ./target/release/hwi-rs
#
# Optional env (one of these must be set, mirroring
# run-core-scenario-coldcard.sh):
#   COLDCARD_SIM_DIR    Path to a built `firmware/unix` directory.
#   COLDCARD_SIM_IMAGE  Podman image with the firmware tree at /work.
#   COLDCARD_SIM_WORK   Host path bind-mounted to /work. Default: $HOME/cc-sim

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BITCOIND="${BITCOIND:-$REPO_ROOT/bitcoin-core/build/bin/bitcoind}"
BITCOIN_CLI="${BITCOIN_CLI:-$REPO_ROOT/bitcoin-core/build/bin/bitcoin-cli}"
export HWI_RS_BIN="${HWI_RS_BIN:-$REPO_ROOT/target/release/hwi-rs}"

for f in "$BITCOIND" "$BITCOIN_CLI" "$HWI_RS_BIN"; do
    if [[ ! -e "$f" ]]; then
        echo "missing file: $f" >&2
        exit 1
    fi
done

if [[ -z "${COLDCARD_SIM_DIR:-}" && -z "${COLDCARD_SIM_IMAGE:-}" ]]; then
    echo "set either COLDCARD_SIM_DIR (native) or COLDCARD_SIM_IMAGE (podman)" >&2
    exit 1
fi

DATADIR="$(mktemp -d)"
SIM_LOG="$DATADIR/coldcard-sim.log"
SOCK_PATH=/tmp/ckcc-simulator.sock
RPCPORT=28453
P2PPORT=28454
SIGNER="$REPO_ROOT/hwi-rs/tests/coldcard-signer.sh"
CONTAINER_NAME="hwi-rs-cc-sim-musig-$$"
WALLET_NAME=musig_hww
POLICY_NAME=MuSigTest

cleanup() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" stop >/dev/null 2>&1 || true
    if [[ -n "${SIM_PID:-}" ]]; then
        kill "$SIM_PID" 2>/dev/null || true
        wait "$SIM_PID" 2>/dev/null || true
    fi
    if [[ -n "${COLDCARD_SIM_IMAGE:-}" ]]; then
        podman rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
    rm -f "$SOCK_PATH"
    sleep 1
    if [[ -z "${KEEP_DATADIR:-}" ]]; then
        rm -rf "$DATADIR"
    else
        echo "KEEP_DATADIR set; leaving $DATADIR in place" >&2
    fi
}
trap cleanup EXIT

rm -f "$SOCK_PATH"

if [[ -n "${COLDCARD_SIM_DIR:-}" ]]; then
    echo "== launching coldcard simulator natively from $COLDCARD_SIM_DIR"
    (
        cd "$COLDCARD_SIM_DIR"
        # `--eff` boots with an ephemeral seed (deterministic master
        # fingerprint 0F056943); `--headless` skips the SDL window.
        exec python3 ./simulator.py --headless --eff
    ) >"$SIM_LOG" 2>&1 &
    SIM_PID=$!
else
    WORK="${COLDCARD_SIM_WORK:-$HOME/cc-sim}"
    echo "== launching coldcard simulator in podman ($COLDCARD_SIM_IMAGE)"
    podman rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    podman run -d --name "$CONTAINER_NAME" \
        -v "$WORK:/work" \
        -v /tmp:/tmp \
        -w /work/firmware/unix \
        "$COLDCARD_SIM_IMAGE" \
        bash -c 'ln -sf ../external/micropython/ports/unix/coldcard-mpy . 2>/dev/null; python3 ./simulator.py --headless --eff' \
        >/dev/null
    SIM_PID=""
fi

echo "== waiting for $SOCK_PATH"
for _ in $(seq 1 60); do
    if [[ -S "$SOCK_PATH" ]]; then
        break
    fi
    sleep 1
done
if [[ ! -S "$SOCK_PATH" ]]; then
    echo "coldcard simulator failed to come up; log:" >&2
    if [[ -n "${COLDCARD_SIM_IMAGE:-}" ]]; then
        podman logs "$CONTAINER_NAME" >&2 || true
    else
        cat "$SIM_LOG" >&2 || true
    fi
    exit 1
fi

echo "== probing simulator via hwi-rs enumerate"
FP_A="$(HWI_RS_COLDCARD_SIMULATOR=1 "$HWI_RS_BIN" enumerate \
    | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 1, f"expected one device, got {entries!r}"
e = entries[0]
assert e.get("error") in (None, ""), f"coldcard error: {e!r}"
fp = e.get("fingerprint")
assert fp, f"no fingerprint reported: {e!r}"
assert e.get("model", "").startswith("coldcard"), f"unexpected model: {e!r}"
print(fp)
')"
echo "coldcard master fingerprint: $FP_A"

echo "== fetching coldcard xpub at m/87h/1h/0h via hwi-rs getxpub"
COSIGNER_A_XPUB="$(HWI_RS_COLDCARD_SIMULATOR=1 "$HWI_RS_BIN" \
        --fingerprint "$FP_A" --chain regtest \
    getxpub "m/87h/1h/0h" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["xpub"])')"
COSIGNER_A_KEY="[${FP_A}/87h/1h/0h]${COSIGNER_A_XPUB}"
echo "cosigner A: $COSIGNER_A_KEY"

echo "== launching bitcoind (regtest) with -signer=$SIGNER"
"$BITCOIND" -regtest -datadir="$DATADIR" -daemon \
    -signer="$SIGNER" \
    -fallbackfee=0.0001 \
    -rpcport="$RPCPORT" -port="$P2PPORT" -listen=0

CLI=("$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT")

echo "== waiting for RPC"
for _ in $(seq 1 30); do
    if "${CLI[@]}" getblockchaininfo >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

wallet_cli() { "${CLI[@]}" -rpcwallet="$WALLET_NAME" "$@"; }
miner_cli() { "${CLI[@]}" -rpcwallet=miner "$@"; }

echo "== createwallet $WALLET_NAME (external_signer=true, blank, private keys enabled)"
"${CLI[@]}" -named createwallet \
    wallet_name="$WALLET_NAME" \
    descriptors=true \
    disable_private_keys=false \
    external_signer=true \
    blank=true >/dev/null

echo "== addhdkey: generate cosigner B's hot HD key inside $WALLET_NAME"
wallet_cli addhdkey >/dev/null

echo "== derivehdkey for cosigner B at m/87h/1h/0h (xpub only; importdescriptors auto-binds the xprv)"
COSIGNER_B_KEY="$(wallet_cli -named derivehdkey \
        path="m/87h/1h/0h" \
    | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["origin"] + v["xpub"])
')"
echo "cosigner B (xpub): $COSIGNER_B_KEY"

DESC_NO_CKSUM="tr(musig(${COSIGNER_A_KEY},${COSIGNER_B_KEY})/<0;1>/*)"
echo "== adding checksum via getdescriptorinfo"
CKSUM="$("${CLI[@]}" getdescriptorinfo "$DESC_NO_CKSUM" \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["checksum"])')"
DESC="${DESC_NO_CKSUM}#${CKSUM}"

echo "== importdescriptors into $WALLET_NAME"
IMPORT_REQ="$(python3 -c "
import json, sys
print(json.dumps([{'desc': sys.argv[1], 'active': True, 'timestamp': 'now'}]))
" "$DESC")"
wallet_cli importdescriptors "$IMPORT_REQ" \
    | python3 -c '
import json, sys
res = json.load(sys.stdin)
for r in res:
    assert r.get("success") is True, f"importdescriptors failed: {r!r}"
'

echo "== getnewaddress (bech32m, derived from imported musig descriptor)"
ADDR="$(wallet_cli getnewaddress "" bech32m)"
echo "receive address: $ADDR"
case "$ADDR" in
    bcrt1p*) ;;
    *) echo "unexpected address format (expected bcrt1p...): $ADDR" >&2; exit 1 ;;
esac

echo "== registerpolicy on $WALLET_NAME as '$POLICY_NAME' (Core -> hwi-rs register -> coldcard simulator)"
REG_OUT="$(wallet_cli registerpolicy "$POLICY_NAME")"
echo "$REG_OUT"
HMAC="$(echo "$REG_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["hmac"])')"
echo "registered hmac: $HMAC"

echo "== getwalletinfo bip388 entry on $WALLET_NAME"
wallet_cli getwalletinfo \
    | FP="$FP_A" HMAC="$HMAC" NAME="$POLICY_NAME" python3 -c '
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

echo "== OK: registered MuSig2 wallet policy via hwi-rs and a Coldcard simulator"
