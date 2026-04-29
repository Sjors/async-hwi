#!/usr/bin/env bash
# Wrapper that pretends to be HWI for hwi-rs's mock mode.
# Used by Bitcoin Core via -signer=<path-to-this-script>.
set -euo pipefail
export HWI_RS_MOCK=1
exec "${HWI_RS_BIN:?HWI_RS_BIN must be set}" "$@"
