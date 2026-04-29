#!/usr/bin/env bash
# Wrapper used by the kumbaya scenario as bitcoind's `-signer=` command.
# Sets BOTH simulator env vars so a single hwi-rs invocation sees the
# Ledger speculos AND the Coldcard simulator at once. `enumerate`
# returns both devices; per-fingerprint subcommands dispatch by master
# fingerprint via the helpers in `hwi-rs/src/devices/dispatch.rs`.
set -euo pipefail
export HWI_RS_LEDGER_SIMULATOR=1
export HWI_RS_COLDCARD_SIMULATOR=1
exec "${HWI_RS_BIN:-hwi-rs}" "$@"
