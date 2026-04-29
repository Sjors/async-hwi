#!/usr/bin/env bash
# Wrapper used by the device-disconnect MuSig2 scenario.
#
# Same contract as speculos-signer.sh by default: forwards every
# subcommand to hwi-rs against a Speculos-backed Ledger app.
#
# When the file $DISCONNECT_FLAG exists, simulates a device that has
# been physically unplugged after Core enumerated it once:
#   * `enumerate` keeps returning the previously-known signer entry
#     (Core caches signers per wallet at attach time anyway, but it
#     re-enumerates inside `send` to find the matching subprocess).
#   * Every other subcommand (signtx, displayaddress, register, ...)
#     emits `{"error": "device disconnected"}` and exits 1, which is
#     what hwi-rs would return if it could no longer reach the device.
#
# Required env (set by the scenario script before launching bitcoind):
#   HWI_RS_BIN          Path to the hwi-rs binary.
#   DISCONNECT_FLAG     Path to the flag file (touched / removed by
#                       the scenario to toggle disconnect state).
#   DEVICE_FINGERPRINT  Master fingerprint of the speculos device, so
#                       the stub enumerate matches what Core stored
#                       on the wallet.
#   DEVICE_MODEL        Optional model string (default: ledger_nano_x).
set -euo pipefail
export HWI_RS_LEDGER_SIMULATOR=1

if [[ -f "${DISCONNECT_FLAG:-/dev/null}" ]]; then
    # Core invokes the signer as `signer-script [global-flags...] subcommand
    # [subcommand-flags...]`, so the subcommand is not necessarily $1.
    # Scan the arg list for the operation we have to special-case.
    has_enumerate=0
    for arg in "$@"; do
        if [[ "$arg" == "enumerate" ]]; then
            has_enumerate=1
            break
        fi
    done
    if (( has_enumerate )); then
        printf '[{"fingerprint":"%s","type":"ledger","model":"%s","path":"speculos"}]\n' \
            "${DEVICE_FINGERPRINT:?DEVICE_FINGERPRINT required when DISCONNECT_FLAG set}" \
            "${DEVICE_MODEL:-ledger_nano_x}"
        exit 0
    fi
    printf '{"error":"device disconnected"}\n'
    exit 1
fi

exec "${HWI_RS_BIN:?HWI_RS_BIN must be set}" "$@"
