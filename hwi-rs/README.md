# `hwi-rs`

A minimal [Bitcoin Core](https://github.com/bitcoin/bitcoin) compatible
**external signer** CLI, built on top of [`async-hwi`](../). It implements the
subset of the Python [HWI](https://github.com/bitcoin-core/HWI) interface that
Bitcoin Core invokes via `-signer=<cmd>`.

This is a **work in progress**. See the tables below for what is currently
wired up.

## Status

### Supported subcommands

| Subcommand                          | Status   | Notes |
| ----------------------------------- | -------- | ----- |

### Supported devices

| Device   | Status                            |
| -------- | --------------------------------- |
| Ledger   | ❌ TODO                           |
| BitBox02 | ❌ TODO                           |
| Coldcard | ❌ TODO                           |
| Jade     | ❌ TODO                           |
| Specter  | ❌ TODO                           |

## Build

From the workspace root:

```sh
cargo build -p hwi-rs --release
```

The binary lives at `target/release/hwi-rs`.

## Why a separate binary?

[`async-hwi-cli`](../cli/) (`hwi`) is a developer/debugging tool with an
ergonomic, human-oriented CLI: subcommand tree, optional flags, logs to
stderr. Bitcoin Core's external-signer interface is rigid: fixed flag names,
strict JSON on stdout, specific exit behavior. Keeping the two binaries
separate avoids a confusing dual-mode tool while reusing every device backend
from `async-hwi` as a library.
