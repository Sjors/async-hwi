#!/usr/bin/env bash
# Wrapper that lets Bitcoin Core invoke hwi-rs against a `coldcard-mpy`
# simulator instead of real USB HID. Pinned -signer for the coldcard
# end-to-end scenario. Used by `tests/run-core-scenario-coldcard.sh`.
set -euo pipefail
export HWI_RS_COLDCARD_SIMULATOR=1
exec "${HWI_RS_BIN:?HWI_RS_BIN must be set}" "$@"
