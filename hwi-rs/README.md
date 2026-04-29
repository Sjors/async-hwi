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
| `enumerate`                         | ✅       | Outputs the HWI JSON shape on stdout. |
| `getdescriptors --account <n>`      | ✅       | Returns `{"receive": [...], "internal": [...]}` (BIP44/49/84/86). |
| `getxpub <path>`                    | ✅       | Returns `{"xpub": "..."}` for a custom BIP32 path (e.g. `m/48'/1'/0'/2'`). |
| `displayaddress --desc <descriptor>`| ✅       | Shows the address on-device; echoes `{"address": "..."}`. |
| `signtx <base64-psbt>`              | ✅       | Signs a PSBT and returns `{"psbt": "..."}`. Use `--stdin` for large PSBTs. |

### Supported devices

| Device   | Status                            |
| -------- | --------------------------------- |
| Ledger   | ✅ (new app only; legacy skipped) |
| BitBox02 | ❌ TODO                           |
| Coldcard | ✅ (Mk4 / Q1, firmware ≥ 6.5.0X)  |
| Jade     | ❌ TODO                           |
| Specter  | ❌ TODO                           |

## Build

From the workspace root:

```sh
cargo build -p hwi-rs --release
```

The binary lives at `target/release/hwi-rs`.

System requirements match `async-hwi`'s `ledger` feature: a working `hidapi`
toolchain (on Linux: `libudev-dev`, `pkg-config`).

## CLI

```text
hwi-rs [--fingerprint <hex>] [--chain {main,test,testnet4,signet,regtest}] <COMMAND>
```

`--chain` matches the strings Bitcoin Core passes (see
`src/util/chaintype.cpp`). `--fingerprint` is required for every subcommand
except `enumerate`.

Output is **JSON on stdout**. On failure, `{"error":"..."}` is printed and the
process exits non-zero — matching what Core's `RunCommandParseJSON` expects.

### Examples

```sh
$ hwi-rs enumerate
[{"type":"ledger","model":"ledger_nano_x","label":null,"path":"...","fingerprint":"00000000","needs_pin_sent":false,"needs_passphrase_sent":false}]
```

## Use with Bitcoin Core

Configure `bitcoind`/`bitcoin-qt` to invoke this binary as the external signer:

```sh
bitcoind -signer=/absolute/path/to/hwi-rs
```

or in `bitcoin.conf`:

```
signer=/absolute/path/to/hwi-rs
```

Then create an external-signer wallet (descriptor wallets only) and import
descriptors from the device:

```sh
$ bitcoin-cli -named createwallet wallet_name="hww" disable_private_keys=true blank=true external_signer=true
$ bitcoin-cli -rpcwallet=hww enumeratesigners
```

`enumeratesigners` will call `hwi-rs enumerate` under the hood.

See
[`doc/external-signer.md`](https://github.com/bitcoin/bitcoin/blob/master/doc/external-signer.md)
in Bitcoin Core for the full protocol.

## Why a separate binary?

[`async-hwi-cli`](../cli/) (`hwi`) is a developer/debugging tool with an
ergonomic, human-oriented CLI: subcommand tree, optional flags, logs to
stderr. Bitcoin Core's external-signer interface is rigid: fixed flag names
(`--chain`, `--fingerprint`, `--stdin`), strict JSON on stdout, specific exit
behavior. Keeping the two binaries separate avoids a confusing dual-mode tool
while reusing every device backend from `async-hwi` as a library.

## Mock mode (CI / local testing)

`hwi-rs` honors a few environment variables that make it act as an
in-process software signer with no real hardware. The mock is backed by
[BIP32 test vector 1](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki#test-vector-1)
(seed `000102030405060708090a0b0c0d0e0f`, master fingerprint `3442193e`).
Because the seed is published, this must never be used for anything outside
testnet/regtest. This is what the CI workflow uses to drive a real
`bitcoind` regtest node end-to-end.

| Variable                    | Default          | Effect |
| --------------------------- | ---------------- | ------ |
| `HWI_RS_MOCK`               | unset            | When `1`, all subcommands respond as the mock signer; HID is not touched. |
| `HWI_RS_MOCK_KIND`          | `ledger`         | Value of the `type` field in `enumerate`. |
| `HWI_RS_MOCK_MODEL`         | `ledger_nano_x`  | Value of `model`. |
| `HWI_RS_LEDGER_SIMULATOR`   | unset            | When `1`, every subcommand connects to a [Speculos](https://github.com/LedgerHQ/speculos) APDU server on `127.0.0.1:9999` instead of touching HID. Used by the speculos integration scenario below. |

The end-to-end scenario script — invoked by CI and runnable locally once
Bitcoin Core has been built — lives at
[`tests/run-core-scenario.sh`](tests/run-core-scenario.sh).

## Speculos mode (CI integration test)

Alongside the mock there is a real-device-flavoured scenario that drives
`hwi-rs` against a [Speculos](https://github.com/LedgerHQ/speculos)
emulator running the official Ledger
[`app-bitcoin-new`](https://github.com/LedgerHQ/app-bitcoin-new) firmware.
The scenario lives at
[`tests/run-core-scenario-speculos.sh`](tests/run-core-scenario-speculos.sh).

Run locally:

```sh
LEDGER_APP_ELF=/path/to/app-bitcoin-new/bin/app.elf \
    bash hwi-rs/tests/run-core-scenario-speculos.sh
```

CI builds the Ledger app .elf in a dedicated job (so the host runner does
not need a Ledger SDK installed) and runs the speculos scenario as part of
the main workflow.
