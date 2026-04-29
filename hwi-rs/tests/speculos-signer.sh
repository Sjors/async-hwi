#!/usr/bin/env bash
# Wrapper that lets Bitcoin Core invoke hwi-rs against a Speculos-backed
# Ledger Bitcoin app instead of real HID. Pinned -signer for the speculos
# end-to-end scenario. Used by `tests/run-core-scenario-speculos.sh`.
set -euo pipefail
export HWI_RS_LEDGER_SIMULATOR=1
exec "${HWI_RS_BIN:?HWI_RS_BIN must be set}" "$@"
