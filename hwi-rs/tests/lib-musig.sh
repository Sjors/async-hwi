#!/usr/bin/env bash
# Shared helpers for the MuSig2 / BIP388 speculos integration scenarios.
#
# Sourced by `run-musig-scenario-speculos.sh` (full happy-path round-trip)
# and `run-musig-disconnect-scenario-speculos.sh` (device-offline mid-flow).
# Both scenarios need an identical scaffold: speculos + an autopressing
# button thread, bitcoind regtest pointed at hwi-rs as -signer, and a
# `musig_hww` wallet holding the Ledger-backed cosigner A and a hot
# cosigner B. Putting that scaffold here keeps each scenario script focused
# on what it's actually exercising.
#
# Caller must `set -euo pipefail` and `set -E` (-E so ERR traps survive
# function calls) before sourcing. Required env vars used below are
# documented in each scenario script's header.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BITCOIND="${BITCOIND:-$REPO_ROOT/bitcoin-core/build/bin/bitcoind}"
BITCOIN_CLI="${BITCOIN_CLI:-$REPO_ROOT/bitcoin-core/build/bin/bitcoin-cli}"
export HWI_RS_BIN="${HWI_RS_BIN:-$REPO_ROOT/target/release/hwi-rs}"
SPECULOS="${SPECULOS:-speculos}"
LEDGER_APP_ELF="${LEDGER_APP_ELF:?LEDGER_APP_ELF must point to the Ledger Bitcoin app .elf}"

SIGNER="$REPO_ROOT/hwi-rs/tests/speculos-signer.sh"
APDU_PORT="${APDU_PORT:-9999}"
SPECULOS_API_PORT="${SPECULOS_API_PORT:-5000}"
RPCPORT="${RPCPORT:-28453}"
P2PPORT="${P2PPORT:-28454}"

POLICY_NAME="MuSigTest"

# Globals populated by helpers below; cleanup uses these.
DATADIR=""
SPECULOS_LOG=""
AUTOPRESS_LOG=""
SPECULOS_PID=""
AUTOPRESS_PID=""

require_files() {
    local f
    for f in "$BITCOIND" "$BITCOIN_CLI" "$HWI_RS_BIN" "$LEDGER_APP_ELF"; do
        if [[ ! -e "$f" ]]; then
            echo "missing file: $f" >&2
            exit 1
        fi
    done
}

setup_datadir() {
    DATADIR="$(mktemp -d)"
    SPECULOS_LOG="$DATADIR/speculos.log"
    AUTOPRESS_LOG="$DATADIR/autopress.log"
}

# Boot speculos with the Ledger Bitcoin app and wait for the APDU port
# to accept connections. Idempotent across stop/start cycles within a
# single scenario, which is what the disconnect test relies on.
start_speculos() {
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
    local _
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
}

stop_speculos() {
    if [[ -n "${SPECULOS_PID:-}" ]]; then
        kill "$SPECULOS_PID" 2>/dev/null || true
        wait "$SPECULOS_PID" 2>/dev/null || true
        SPECULOS_PID=""
    fi
    # Wait for the APDU port to actually go away so the next hwi-rs
    # call doesn't race a still-listening socket.
    local _
    for _ in $(seq 1 20); do
        if ! (echo > "/dev/tcp/127.0.0.1/$APDU_PORT") 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    echo "warning: speculos APDU port still open after stop" >&2
}

# Background loop that mashes the right and both buttons on the
# emulated Ledger via the speculos REST API. Used to auto-confirm
# prompts during getxpub / register / displayaddress / signtx. Stop
# before any RPC call that should *not* press buttons.
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

cleanup_all() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" stop >/dev/null 2>&1 || true
    stop_autopress
    stop_speculos
    sleep 1
    if [[ -z "${KEEP_DATADIR:-}" ]]; then
        rm -rf "$DATADIR"
    else
        echo "KEEP_DATADIR set; leaving $DATADIR in place" >&2
    fi
}

# Probe the running speculos via hwi-rs enumerate and print the
# device's master fingerprint to stdout (one line, no newline noise).
get_speculos_fingerprint() {
    HWI_RS_LEDGER_SIMULATOR=1 "$HWI_RS_BIN" enumerate \
        | python3 -c '
import json, sys
entries = json.load(sys.stdin)
assert len(entries) == 1, f"expected one device, got {entries!r}"
print(entries[0]["fingerprint"])
'
}

# Fetch xpub at $2 from the speculos device with fingerprint $1, with
# autopress wrapping the call (BIP87 is outside the Ledger app's
# standard-path whitelist so the device prompts).
get_speculos_xpub() {
    local fp="$1" path="$2" xpub
    start_autopress
    xpub="$(HWI_RS_LEDGER_SIMULATOR=1 "$HWI_RS_BIN" \
            --fingerprint "$fp" --chain test \
        getxpub "$path" | python3 -c 'import json,sys; print(json.load(sys.stdin)["xpub"])')"
    stop_autopress
    printf '%s' "$xpub"
}

start_bitcoind() {
    echo "== launching bitcoind (regtest) with -signer=$SIGNER"
    "$BITCOIND" -regtest -datadir="$DATADIR" -daemon \
        -signer="$SIGNER" \
        -fallbackfee=0.0001 \
        -rpcport="$RPCPORT" -port="$P2PPORT" -listen=0

    echo "== waiting for RPC"
    local _
    for _ in $(seq 1 30); do
        if "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" \
                getblockchaininfo >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "bitcoind RPC did not come up" >&2
    exit 1
}

# Wrappers around bitcoin-cli for brevity in the scenario scripts.
core_cli() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" "$@"
}

wallet_cli() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" \
        -rpcwallet="${WALLET_NAME:-musig_hww}" "$@"
}

# Same as wallet_cli but with an explicit wallet name as $1, for
# scenarios that juggle more than one Core wallet.
wallet_cli_for() {
    local w="$1"; shift
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" \
        -rpcwallet="$w" "$@"
}

# Wallet that scenarios default to (overridable per scenario).
WALLET_NAME="musig_hww"
DESC=""
COSIGNER_B_KEY=""

# Create an external-signer-backed Core wallet with one hot HD key for
# cosigner B at m/87h/1h/0h, and populate $COSIGNER_B_KEY (xpub form,
# with origin) for use in the musig() descriptor. Importing the
# descriptor with B in xpub-only form works because
# `importdescriptors` auto-binds wallet-known xprvs by master
# fingerprint at import time.
#
# Flags rationale:
#   - external_signer=true so registerpolicy can find an SPKM later
#   - disable_private_keys=false so addhdkey works
#   - blank=true so Core skips its 8 BIP44/49/84/86 device descriptors
#     (we only want the musig() one) and skips internal HD-seeding
#     (so derivehdkey doesn't need an `hdkey=` disambiguator)
create_signer_wallet_with_hot_b() {
    local wallet="${1:-$WALLET_NAME}"

    echo "== createwallet $wallet (external_signer=true, blank, private keys enabled)"
    core_cli -named createwallet \
        wallet_name="$wallet" \
        descriptors=true \
        disable_private_keys=false \
        external_signer=true \
        blank=true >/dev/null

    echo "== addhdkey: generate cosigner B's hot HD key inside $wallet"
    wallet_cli_for "$wallet" addhdkey >/dev/null

    echo "== derivehdkey for cosigner B at m/87h/1h/0h (xpub only; importdescriptors auto-binds the xprv)"
    COSIGNER_B_KEY="$(wallet_cli_for "$wallet" -named derivehdkey \
            path="m/87h/1h/0h" \
        | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["origin"] + v["xpub"])
')"
    echo "cosigner B (xpub): $COSIGNER_B_KEY"
}

# Create an external-signer-backed Core wallet with no private keys.
# Used for the watch-only side of the timelock scenario, where B's
# xprv lives in another wallet and this one only ever sees the
# device-signed (or timelock-tapscript-signed) half.
create_signer_watchonly_wallet() {
    local wallet="${1:-$WALLET_NAME}"

    echo "== createwallet $wallet (external_signer=true, blank, watch-only)"
    core_cli -named createwallet \
        wallet_name="$wallet" \
        descriptors=true \
        disable_private_keys=true \
        external_signer=true \
        blank=true >/dev/null
}

# Add a checksum to the descriptor template $1, importdescriptors it as
# active into wallet $2 (default $WALLET_NAME). Sets the global $DESC.
import_active_descriptor() {
    local desc_no_cksum="$1" wallet="${2:-$WALLET_NAME}"

    echo "== adding checksum via getdescriptorinfo"
    local cksum
    cksum="$(core_cli getdescriptorinfo "$desc_no_cksum" \
        | python3 -c 'import json,sys;print(json.load(sys.stdin)["checksum"])')"
    DESC="${desc_no_cksum}#${cksum}"

    echo "== importdescriptors into $wallet"
    local import_req
    import_req="$(python3 -c "
import json, sys
print(json.dumps([{'desc': sys.argv[1], 'active': True, 'timestamp': 'now'}]))
" "$DESC")"
    wallet_cli_for "$wallet" importdescriptors "$import_req" \
        | python3 -c '
import json, sys
res = json.load(sys.stdin)
for r in res:
    assert r.get("success") is True, f"importdescriptors failed: {r!r}"
'
}

# Backwards-compatible thin wrapper: build the simple 2-of-2 MuSig2
# descriptor (no script path) and import it into $WALLET_NAME. B is
# in xpub-only form; importdescriptors auto-binds the xprv from the
# `addhdkey` seed.
setup_musig_wallet() {
    local cosigner_a_key="$1"
    create_signer_wallet_with_hot_b "$WALLET_NAME"
    import_active_descriptor \
        "tr(musig(${cosigner_a_key},${COSIGNER_B_KEY})/<0;1>/*)" \
        "$WALLET_NAME"
}

# Register the imported policy with the device, asserting that the
# returned hmac matches what Core persists in getwalletinfo.bip388.
# Optional positional args: $2 = wallet name (default $WALLET_NAME),
# $3 = policy name (default $POLICY_NAME).
register_musig_policy() {
    local fp="$1" wallet="${2:-$WALLET_NAME}" policy_name="${3:-$POLICY_NAME}"
    echo "== registerpolicy on $wallet as '$policy_name' (Core -> hwi-rs register -> speculos), autoclicking"
    start_autopress
    local reg_out hmac
    reg_out="$(wallet_cli_for "$wallet" registerpolicy "$policy_name")"
    stop_autopress
    echo "$reg_out"
    hmac="$(echo "$reg_out" | python3 -c 'import json,sys;print(json.load(sys.stdin)["hmac"])')"
    echo "registered hmac: $hmac"

    echo "== getwalletinfo bip388 entry on $wallet"
    wallet_cli_for "$wallet" getwalletinfo \
        | FP="$fp" HMAC="$hmac" NAME="$policy_name" python3 -c '
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
}

# Create a helper miner wallet and mature 101 blocks to it; exposes
# $MINER_ADDR for the scenario script to use as fund / change source.
MINER_ADDR=""
setup_miner_wallet() {
    echo "== creating helper miner wallet (no external signer)"
    core_cli -named createwallet \
        wallet_name=miner \
        descriptors=true \
        blank=false >/dev/null
    MINER_ADDR="$(core_cli -rpcwallet=miner getnewaddress "" bech32m)"
    echo "== mining 101 blocks to miner so it has spendable coinbase"
    core_cli -rpcwallet=miner generatetoaddress 101 "$MINER_ADDR" >/dev/null
}

miner_cli() {
    "$BITCOIN_CLI" -regtest -datadir="$DATADIR" -rpcport="$RPCPORT" \
        -rpcwallet=miner "$@"
}
