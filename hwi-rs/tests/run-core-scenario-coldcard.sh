#!/usr/bin/env bash
# End-to-end test: drive the Coldcard `coldcard-mpy` simulator from
# `hwi-rs` directly (no bitcoind yet — that comes once enough
# subcommands are wired up to make a useful Core round trip).
# Counterpart to run-core-scenario-speculos.sh (Ledger) and
# run-core-scenario.sh (in-process software mock).
#
# The Coldcard simulator is a Linux ELF built from the Coldcard firmware
# tree (see AGENTS.md) that talks ckcc protocol over a unix datagram
# socket at /tmp/ckcc-simulator.sock. We can run it natively if the host
# has a working SDL2 + headless setup, or inside a Podman container that
# was prepared by the `coldcard_sim` CI job.
#
# Required env (set automatically in CI):
#   BITCOIND        Path to bitcoind. Default: ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI     Path to bitcoin-cli. Default: ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN      Path to hwi-rs binary. Default: ./target/release/hwi-rs
#
# Optional env:
#   COLDCARD_SIM_DIR    Path to a built `firmware/unix` directory that
#                       contains `simulator.py` and the `coldcard-mpy`
#                       binary. If set, the simulator is launched
#                       natively with `python3 simulator.py --headless`.
#   COLDCARD_SIM_IMAGE  Podman image name (built by AGENTS.md / CI) that
#                       contains the firmware tree at /work/firmware.
#                       If set and COLDCARD_SIM_DIR is not, the
#                       simulator is launched in a container.
#   COLDCARD_SIM_WORK   Host path bind-mounted to /work in the
#                       container. Default: $HOME/cc-sim
#
# Exactly one of COLDCARD_SIM_DIR / COLDCARD_SIM_IMAGE must be set.

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
SIGNER="$REPO_ROOT/hwi-rs/tests/coldcard-signer.sh"
CONTAINER_NAME="hwi-rs-cc-sim-$$"

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
    rm -rf "$DATADIR"
}
trap cleanup EXIT

# Make sure no stale socket / container is in the way.
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
    # Bind-mount /tmp so the simulator's unix socket is visible on the host.
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
ENUM_RAW="$(HWI_RS_COLDCARD_SIMULATOR=1 "$HWI_RS_BIN" enumerate)"
echo "$ENUM_RAW"
FP="$(echo "$ENUM_RAW" | python3 -c '
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
echo "== probing simulator via hwi-rs getdescriptors (regtest, account 0)"
DESC_RAW="$(HWI_RS_COLDCARD_SIMULATOR=1 "$HWI_RS_BIN" --fingerprint "$FP" --chain regtest getdescriptors --account 0)"
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
    -rpcport="$RPCPORT" -port=28454 -listen=0

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

echo "== walletdisplayaddress (Core -> hwi-rs displayaddress -> coldcard simulator)"
DISP="$("${CLI[@]}" -rpcwallet=hww walletdisplayaddress "$ADDR")"
echo "$DISP"
echo "$DISP" | ADDR="$ADDR" python3 -c '
import json, os, sys
got = json.load(sys.stdin).get("address")
want = os.environ["ADDR"]
assert got == want, f"walletdisplayaddress returned {got!r}, expected {want!r}"
'

echo "== OK"
