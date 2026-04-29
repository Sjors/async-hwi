#!/usr/bin/env bash
# "Kumbaya" 3-of-3 MuSig2/BIP388 scenario: combine BOTH supported
# hardware-wallet simulators (Ledger speculos + Coldcard `coldcard-mpy`)
# and a Bitcoin Core hot HD key into a single MuSig2 wallet, then drive
# a full spend through `send` in one RPC call.
#
# Layout:
#   * Cosigner A: speculos-emulated Ledger Bitcoin app (master
#                 fingerprint f5acc2fd)
#   * Cosigner B: `coldcard-mpy` simulator booted with --eff (master
#                 fingerprint 0F056943)
#   * Cosigner C: hot HD key inside the Core wallet (created via
#                 `addhdkey`)
#
# bitcoind is started with -signer pointing at kumbaya-signer.sh, which
# sets both HWI_RS_LEDGER_SIMULATOR=1 and HWI_RS_COLDCARD_SIMULATOR=1.
# `enumerate` then returns both devices, and per-fingerprint subcommands
# (getxpub, register, displayaddress, signtx) dispatch by `--fingerprint`
# inside hwi-rs (see `hwi-rs/src/devices/dispatch.rs`).
#
# Required env (defaults match the CI workflow):
#   BITCOIND        Path to MuSig2/BIP388-capable bitcoind (Sjors's
#                   2025/06/musig2-power branch). Default:
#                   ./bitcoin-core/build/bin/bitcoind
#   BITCOIN_CLI     Matching bitcoin-cli. Default:
#                   ./bitcoin-core/build/bin/bitcoin-cli
#   HWI_RS_BIN      Path to hwi-rs binary. Default: ./target/release/hwi-rs
#   LEDGER_APP_ELF  Path to the Ledger Bitcoin app .elf. Required.
#   SPECULOS        Path to the speculos entry point. Default: speculos
#
# Optional env (one of these must be set, mirroring the Coldcard
# scenarios):
#   COLDCARD_SIM_DIR    Path to a built `firmware/unix` directory.
#   COLDCARD_SIM_IMAGE  Podman image with the firmware tree at /work.
#   COLDCARD_SIM_WORK   Host path bind-mounted to /work (default: $HOME/cc-sim).

set -Eeuo pipefail

# Reuse the speculos / wallet helpers; we only override the signer
# path and add a bit of Coldcard-side scaffolding.
# shellcheck source=lib-musig.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-musig.sh"

if [[ -z "${COLDCARD_SIM_DIR:-}" && -z "${COLDCARD_SIM_IMAGE:-}" ]]; then
    echo "set either COLDCARD_SIM_DIR (native) or COLDCARD_SIM_IMAGE (podman)" >&2
    exit 1
fi

# Override the signer wrapper used by start_bitcoind / wallet_cli (the
# default in lib-musig.sh points at the Ledger-only wrapper).
SIGNER="$(dirname "${BASH_SOURCE[0]}")/kumbaya-signer.sh"

WALLET_NAME="musig_kumbaya"
POLICY_NAME="MuSigKumbaya"
SOCK_PATH=/tmp/ckcc-simulator.sock
CC_CONTAINER_NAME="hwi-rs-cc-sim-kumbaya-$$"
CC_SIM_PID=""

require_files
setup_datadir
SIM_LOG="$DATADIR/coldcard-sim.log"

cleanup_kumbaya() {
    cleanup_all
    if [[ -n "${CC_SIM_PID:-}" ]]; then
        kill "$CC_SIM_PID" 2>/dev/null || true
        wait "$CC_SIM_PID" 2>/dev/null || true
    fi
    if [[ -n "${COLDCARD_SIM_IMAGE:-}" ]]; then
        podman rm -f "$CC_CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
    rm -f "$SOCK_PATH" 2>/dev/null || true
}
trap cleanup_kumbaya EXIT

# --- Boot Coldcard simulator ----------------------------------------------
rm -f "$SOCK_PATH"
if [[ -n "${COLDCARD_SIM_DIR:-}" ]]; then
    echo "== launching coldcard simulator natively from $COLDCARD_SIM_DIR"
    (
        cd "$COLDCARD_SIM_DIR"
        exec python3 ./simulator.py --headless --eff
    ) >"$SIM_LOG" 2>&1 &
    CC_SIM_PID=$!
else
    WORK="${COLDCARD_SIM_WORK:-$HOME/cc-sim}"
    echo "== launching coldcard simulator in podman ($COLDCARD_SIM_IMAGE)"
    podman rm -f "$CC_CONTAINER_NAME" >/dev/null 2>&1 || true
    podman run -d --name "$CC_CONTAINER_NAME" \
        -v "$WORK:/work" \
        -v /tmp:/tmp \
        -w /work/firmware/unix \
        "$COLDCARD_SIM_IMAGE" \
        bash -c 'ln -sf ../external/micropython/ports/unix/coldcard-mpy . 2>/dev/null; python3 ./simulator.py --headless --eff' \
        >/dev/null
fi

echo "== waiting for $SOCK_PATH"
for _ in $(seq 1 60); do
    [[ -S "$SOCK_PATH" ]] && break
    sleep 1
done
[[ -S "$SOCK_PATH" ]] || {
    echo "coldcard simulator failed to come up; log:" >&2
    if [[ -n "${COLDCARD_SIM_IMAGE:-}" ]]; then
        podman logs "$CC_CONTAINER_NAME" >&2 || true
    else
        cat "$SIM_LOG" >&2 || true
    fi
    exit 1
}

# --- Boot speculos --------------------------------------------------------
start_speculos

# --- Probe both devices via hwi-rs (with both sim env vars set) ----------
echo "== probing both simulators via hwi-rs enumerate (kumbaya mode)"
ENUM_RAW="$(HWI_RS_LEDGER_SIMULATOR=1 HWI_RS_COLDCARD_SIMULATOR=1 \
    "$HWI_RS_BIN" enumerate)"
echo "$ENUM_RAW"
read -r FP_LEDGER FP_COLDCARD <<<"$(echo "$ENUM_RAW" | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 2, f"expected two devices, got {entries!r}"
ledger = next((e for e in entries if e["type"] == "ledger"), None)
coldcard = next((e for e in entries if e["type"] == "coldcard"), None)
assert ledger and coldcard, f"missing ledger/coldcard: {entries!r}"
assert ledger.get("error") in (None, ""), ledger
assert coldcard.get("error") in (None, ""), coldcard
print(ledger["fingerprint"], coldcard["fingerprint"])
')"
echo "ledger fingerprint:   $FP_LEDGER"
echo "coldcard fingerprint: $FP_COLDCARD"

# --- Fetch each device's xpub at m/87'/1'/0' -----------------------------
echo "== fetching speculos xpub via hwi-rs getxpub (autoclick required)"
COSIGNER_A_XPUB="$(get_speculos_xpub "$FP_LEDGER" "m/87'/1'/0'")"
COSIGNER_A_KEY="[${FP_LEDGER}/87h/1h/0h]${COSIGNER_A_XPUB}"
echo "cosigner A (ledger): $COSIGNER_A_KEY"

echo "== fetching coldcard xpub via hwi-rs getxpub"
# Both simulator env vars set, so dispatch by fingerprint inside hwi-rs.
COSIGNER_B_XPUB="$(HWI_RS_LEDGER_SIMULATOR=1 HWI_RS_COLDCARD_SIMULATOR=1 \
    "$HWI_RS_BIN" --fingerprint "$FP_COLDCARD" --chain regtest \
    getxpub "m/87h/1h/0h" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["xpub"])')"
COSIGNER_B_KEY="[${FP_COLDCARD}/87h/1h/0h]${COSIGNER_B_XPUB}"
echo "cosigner B (coldcard): $COSIGNER_B_KEY"

# --- Boot bitcoind with the kumbaya signer wrapper -----------------------
start_bitcoind

# --- Build the wallet: external-signer + a Core-hot cosigner C -----------
# create_signer_wallet_with_hot_b derives the hot HD key at m/87h/1h/0h
# and writes its origin+xpub into $COSIGNER_B_KEY. In kumbaya layout
# that's our cosigner C; rebind for clarity in the descriptor below.
echo "== creating $WALLET_NAME with cosigner C as the hot HD key"
create_signer_wallet_with_hot_b "$WALLET_NAME"
COSIGNER_C_KEY="$COSIGNER_B_KEY"
echo "cosigner C (core hot): $COSIGNER_C_KEY"

# Cosigner B is the Coldcard xpub fetched above; rebind for clarity.
COSIGNER_B_KEY="[${FP_COLDCARD}/87h/1h/0h]${COSIGNER_B_XPUB}"

DESC_NO_CKSUM="tr(musig(${COSIGNER_A_KEY},${COSIGNER_B_KEY},${COSIGNER_C_KEY})/<0;1>/*)"
import_active_descriptor "$DESC_NO_CKSUM" "$WALLET_NAME"

# Re-derive the receive address for assertions.
ADDR="$(wallet_cli getnewaddress "" bech32m)"
echo "receive address: $ADDR"
case "$ADDR" in
    bcrt1p*) ;;
    *) echo "unexpected address format (expected bcrt1p...): $ADDR" >&2; exit 1 ;;
esac

# --- Register the policy on BOTH devices ---------------------------------
echo "== registerpolicy on $WALLET_NAME (Core fans out to both signers)"
start_autopress
REG_OUT="$(wallet_cli registerpolicy "$POLICY_NAME")"
stop_autopress
echo "$REG_OUT"

echo "== getwalletinfo bip388: expect one entry per signer fingerprint"
wallet_cli getwalletinfo \
    | FP_LEDGER="$FP_LEDGER" FP_COLDCARD="$FP_COLDCARD" NAME="$POLICY_NAME" python3 -c '
import json, os, sys
w = json.load(sys.stdin)
hmacs = w.get("bip388", [])
assert hmacs, f"no bip388 hmacs in getwalletinfo: {w!r}"
fps = {h["fingerprint"] for h in hmacs if h["name"] == os.environ["NAME"]}
expected = {os.environ["FP_LEDGER"], os.environ["FP_COLDCARD"]}
assert fps == expected, f"bip388 fingerprints {fps!r} != {expected!r}"
# Ledger returns an hmac, Coldcard does not.
ledger = next(h for h in hmacs if h["fingerprint"] == os.environ["FP_LEDGER"])
coldcard = next(h for h in hmacs if h["fingerprint"] == os.environ["FP_COLDCARD"])
assert "hmac" in ledger, f"missing ledger hmac: {ledger!r}"
assert "hmac" not in coldcard, f"unexpected coldcard hmac: {coldcard!r}"
'

# --- walletdisplayaddress drives just one device (first match wins) ------
echo "== walletdisplayaddress (Core picks first matching signer)"
start_autopress
WDA_OUT="$(wallet_cli walletdisplayaddress "$ADDR")"
stop_autopress
echo "$WDA_OUT"
WDA_ADDR="$(echo "$WDA_OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["address"])')"
[[ "$WDA_ADDR" == "$ADDR" ]] || { echo "walletdisplayaddress echoed unexpected address: $WDA_ADDR" >&2; exit 1; }

# --- Funding + 3-of-3 MuSig2 spend in one `send` call --------------------
setup_miner_wallet
echo "== funding $WALLET_NAME receive address $ADDR with 1.0 BTC"
miner_cli -named sendtoaddress address="$ADDR" amount=1.0 >/dev/null
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null

BAL="$(wallet_cli getbalance)"
echo "$WALLET_NAME balance: $BAL"
python3 - <<PY
b = float("$BAL")
assert b >= 0.999, f"unexpected $WALLET_NAME balance: {b}"
PY

DEST_ADDR="$(miner_cli getnewaddress "" bech32m)"

echo "== send (single call: round 1 + round 2 across all 3 cosigners)"
# Two HW devices each get 2 prompts (round 1 nonce + round 2 partial sig)
# plus the local (Core hot) cosigner contributes from the wallet's xprv.
# The Ledger autopressing thread keeps the speculos UI moving; the
# Coldcard simulator approves on its own via XKEY-injected `y` keypresses
# inside hwi-rs.
start_autopress
SEND_OUT="$(wallet_cli -named send \
    outputs="[{\"$DEST_ADDR\": 0.5}]" \
    fee_rate=5)"
stop_autopress
echo "$SEND_OUT"
COMPLETE="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["complete"])')"
[[ "$COMPLETE" == "True" ]] || { echo "send did not complete in one call: $SEND_OUT" >&2; exit 1; }
SPEND_TXID="$(echo "$SEND_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["txid"])')"
echo "spend txid: $SPEND_TXID"

echo "== mine a confirmation block and verify the spend confirmed"
miner_cli generatetoaddress 1 "$MINER_ADDR" >/dev/null
wallet_cli gettransaction "$SPEND_TXID" \
    | python3 -c 'import json,sys;t=json.load(sys.stdin);assert t["confirmations"] >= 1, t;print("confirmations:", t["confirmations"])'

echo "== OK: 3-of-3 MuSig2 (Ledger + Coldcard + Core hot key) in one send"
