# `hwi-rs` usage: MuSig2 with Bitcoin Core

This is the manual version of what `hwi-rs/tests/run-musig-scenario-speculos.sh`
automates. It sets up a 2-of-2 MuSig2 wallet on **testnet4** where:

- **Cosigner A** lives on a Ledger (real device running the Bitcoin
  Testnet app, or speculos with the same app .elf).
- **Cosigner B** is a hot key in the same Bitcoin Core wallet, which
  also holds the watch-only MuSig descriptor.

## 1. Build Bitcoin Core

The required RPCs (`derivehdkey`, `registerpolicy`, `getwalletinfo.bip388`,
`tr(musig(...))` descriptors, `<0;1>` multipath in `importdescriptors`) are
not in master yet. Build Sjors's branch:

```bash
git clone https://github.com/Sjors/bitcoin.git
cd bitcoin
git checkout 2025/06/musig2-power
cmake -B build
cmake --build build
```

## 2. Build `hwi-rs`

```bash
git clone https://github.com/wizardsardine/async-hwi.git
cd async-hwi
cargo build -p hwi-rs
# The (debug) binary is at ./target/debug/hwi-rs.
```

## 3. Start `bitcoind` with `hwi-rs` as the external signer

`bitcoind` invokes the `-signer` binary with the same JSON protocol HWI
speaks (`enumerate`, `getdescriptors`, `displayaddress`, `signtx`,
`register`). Point it straight at the `hwi-rs` binary — no wrapper
script needed.

Launch `bitcoind` in its own terminal so you can keep an eye on its logs:

```bash
bitcoind -testnet4 \
    -signer=/path/to/async-hwi/target/debug/hwi-rs
```

Then, in a second terminal, talk to it with `bitcoin-cli`:

```bash
bitcoin-cli -testnet4 -getinfo   # wait for RPC to come up
```

Confirm the device is visible to Core:

```bash
bitcoin-cli -testnet4 enumeratesigners
# {
#   "signers": [
#     { "fingerprint": "f5acc2fd", "name": "ledger_nano_x" }
#   ]
# }
```

Note the fingerprint — call it `$FP_A`.

## 4. Cosigner A: pull the Ledger xpub

For MuSig2 we use [BIP87](https://github.com/bitcoin/bips/blob/master/bip-0087.mediawiki)
account keys (`m/87'/coin_type'/account'`). BIP87 is the script-type-
agnostic multisig purpose; BIP48 was designed for fixed-set legacy
multisig with explicit pubkey lists and has no Taproot variant.
`getdescriptors` only covers the canonical BIP44/49/84/86 paths, so
use the dedicated `getxpub` subcommand to fetch a single xpub at a
custom path. BIP87 is outside the Ledger Bitcoin app's standard-path
whitelist, so the device prompts for confirmation:

```bash
./target/debug/hwi-rs --fingerprint "$FP_A" --chain testnet4 \
    getxpub "m/87'/1'/0'"
# {"xpub": "tpubDC..."}
```

The Ledger Bitcoin app does not distinguish between `test`,
`testnet4`, `regtest` and `signet` on the device side — they all map
to the same chain identifier as far as the app is concerned — so the
exact `--chain` value passed here only affects how `hwi-rs` itself
formats addresses for return.

Build the key expression with origin info (Core needs both the fingerprint
and derivation path on every cosigner key):

```bash
COSIGNER_A_KEY="[${FP_A}/87h/1h/0h]tpubDC...."
```

## 5. Create the MuSig2 Core wallet

A single wallet holds cosigner A's external-signer descriptors *and*
cosigner B's hot key. We pass:

- `external_signer=true` so the wallet is signer-aware from birth and
  `registerpolicy` in step 9 finds an `ExternalSignerScriptPubKeyMan`
  to drive.
- `disable_private_keys=false` (overriding the external-signer default
  of true), so we can `addhdkey` cosigner B's hot HD seed and import
  the `tr(musig(...))` descriptor with B's xprv inline.
- `blank=true` so Core skips its default BIP44/49/84/86
  device-descriptor auto-import. We never derive from those eight
  xpub-only descriptors — we only want one descriptor in this wallet,
  the `tr(musig(...))` one — so they'd just be unbacked-up extra key
  material on disk, and they'd make `derivehdkey` ambiguous in step 6.
  `blank=true` also skips Core's default internal HD-key seeding.

```bash
bitcoin rpc -testnet4 createwallet \
    wallet_name=musig_hww \
    descriptors=true \
    disable_private_keys=false \
    external_signer=true \
    blank=true
```

This can equivalently be done from the GUI's *File → Create Wallet*
dialog (tick *External signer*, untick *Disable private keys*, tick
*Make blank wallet*).

## 6. Cosigner B: hot key inside `musig_hww`, exported via `derivehdkey`

Mint cosigner B's hot HD key with `addhdkey`, then derive the BIP87
account *xpub* with `derivehdkey`. Because we created the wallet
`blank=true`, the only HD key around is the one `addhdkey` just
generated, and `derivehdkey` doesn't need an `hdkey=` disambiguator.
We don't need the xprv form here: at `importdescriptors` time Core
auto-binds the xprv from the wallet's known seeds whenever the
descriptor's master fingerprint matches:

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww addhdkey
# {"xpub": "tpubD6..."}

./build/bin/bitcoin rpc -testnet4 -rpcwallet=musig_hww derivehdkey \
    path="m/87h/1h/0h"
# {
#   "origin": "[abcd1234/87h/1h/0h]",
#   "xpub":   "tpubDE..."
# }

FP_B=abcd1234                                  # from the origin field above
COSIGNER_B_KEY="[${FP_B}/87h/1h/0h]tpubDE...." # xpub form, used for the descriptor
```

`derivehdkey` is new in this branch — it returns the key *plus* the origin
string already in the right format. No client-side BIP32 derivation needed.

## 7. Build the MuSig2 descriptor and checksum it

We use one multipath descriptor (`<0;1>`) so Core sets up matching receive
(`/0/*`) and change (`/1/*`) script-pubkey managers from a single import.
Both cosigners are in xpub-only form: A because its xprv lives on the
device, B because `importdescriptors` will bind the wallet's known xprv
automatically by master fingerprint.

```bash
DESC_NO_CKSUM="tr(musig(${COSIGNER_A_KEY},${COSIGNER_B_KEY})/<0;1>/*)"

CKSUM=$(bitcoin-cli -testnet4 getdescriptorinfo "$DESC_NO_CKSUM" | jq -r .checksum)
DESC="${DESC_NO_CKSUM}#${CKSUM}"
```

## 8. Import the descriptor and derive a receive address

`importdescriptors` has no GUI affordance yet (see
[bitcoin/bitcoin#34861](https://github.com/bitcoin/bitcoin/pull/34861)),
so run it from `bitcoin-cli` or the GUI's *Window → Console*:

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww importdescriptors \
    "[{\"desc\": \"${DESC}\", \"active\": true, \"timestamp\": \"now\"}]"
# [{"success":true, "warnings":[...]}]
```

The two warnings Core prints — `"Range not given, using default keypool
range"` and `"Not all private keys provided. Some wallet functionality
may return unexpected errors"` — are both expected here. The first is
because we didn't pass `range`; the second is because the MuSig2
aggregate xpub has no private key by construction (and cosigner A's
share lives on the device, not in the wallet — which is the whole
point of the external-signer setup). `importdescriptors` quietly binds
B's xprv from the wallet's `addhdkey` seed by master fingerprint, so
the wallet can still co-sign the key path locally.

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww getnewaddress "" bech32m
# tb1p...    <- a P2TR MuSig2 address
```

`getnewaddress` also works from the GUI's *Receive* tab.

## 9. Register the policy on the device (BIP388)

`registerpolicy` walks the active MuSig2 descriptor pair, calls `hwi-rs
register` over the external-signer pipe, and returns a 32-byte HMAC the
device gives us back to prove enrolment. Confirm on the screen.

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww registerpolicy MuSigTest
# {"hmac":"85d68dad...30949f9c"}
```

The HMAC is also persisted in the wallet:

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww getwalletinfo | jq '.bip388'
# [
#   {
#     "name": "MuSigTest",
#     "fingerprint": "f5acc2fd",
#     "hmac": "85d68dad...30949f9c"
#   }
# ]
```

## 10. Display a registered address on the Ledger

`walletdisplayaddress` looks the address up in the wallet, finds the
matching `ExternalSignerScriptPubKeyMan`, recognises that it backs a
registered BIP388 policy, and dispatches the on-device display through
that policy (template + keys + hmac) instead of through the single-key
`InferDescriptor` path. The device confirms the address on its screen
and Core checks that the echoed address resolves to the same
`scriptPubKey` it asked about.

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww walletdisplayaddress tb1p...
# {"address":"tb1p..."}
```

The GUI exposes the same flow from the *Receive* tab: pick (or
generate) an address in the request history, double-click it to open
the request dialog, and click *Verify*. The button only appears for
external-signer wallets and dispatches through the same
`displayAddress` path.

The Ledger screen shows the same address (modulo HRP \u2014 the Bitcoin
Testnet app encodes regtest addresses with the `tb1` testnet prefix,
which Core's `walletdisplayaddress` accepts as long as the underlying
witness program matches).

## 11. Spending

You'll need to fund the MuSig2 address from a separate wallet. We'll then
spend it back through MuSig2.

### One-shot: `send` runs both rounds

MuSig2 needs two signing rounds — round 1 exchanges public nonces,
round 2 produces partial signatures that aggregate to a single
Schnorr key-path signature. Because both cosigners live in the same
`musig_hww` wallet here (cosigner A on the Ledger, cosigner B as a
hot key), Core can complete both rounds back-to-back inside `send`.
Round 1 (nonce exchange) doesn't prompt — confirm once on the device
when round 2 asks for the partial signature:

```bash
DEST_ADDR=tb1p...     # any destination

SEND_OUT="$(bitcoin-cli -testnet4 -rpcwallet=musig_hww -named send \
    outputs="[{\"$DEST_ADDR\": 0.0005}]" \
    fee_rate=1)"
echo "$SEND_OUT" | jq '.complete'   # true
TXID="$(echo "$SEND_OUT" | jq -r .txid)"
```

In the GUI use the *Send* tab — fill in the destination and amount,
click *Send*, and confirm on the device when round 2 prompts.

Wait for a confirmation and check it landed:

```bash
bitcoin-cli -testnet4 -rpcwallet=musig_hww gettransaction "$TXID" | jq '.confirmations'
# 1
```

The *Transactions* tab shows the same entry once it confirms.

> **Two-round fallback.** When some cosigner can't sign in-process —
> e.g. their key lives in a different wallet, or on an offline device
> — `send` returns `complete=false` with the round-1 PSBT (containing
> just the locally-known cosigner's pub nonce, BIP-373 input field
> `0x1b`). Ferry that PSBT to the missing cosigner, have them
> contribute their nonce *and* partial signature with
> `walletprocesspsbt psbt=... sign=true finalize=true`, then on the
> resulting `complete=true` PSBT run `finalizepsbt` to extract the
> hex and `sendrawtransaction` to broadcast. You can inspect the
> nonces and partial sigs at any stage with
> `decodepsbt | jq '.inputs[].musig2_pubnonces, .inputs[].musig2_partial_sigs'`.

For the fully-automated regtest version (helper miner wallet,
auto-clicking speculos, single-call `send`, and assertions on the
broadcast tx), see
[`tests/run-musig-scenario-speculos.sh`](tests/run-musig-scenario-speculos.sh).

> **Device unplugged before `send`.** The two-round fallback above
> also covers the case where the Ledger isn't attached when `send`
> runs (e.g. signer at a different location).
> `send` returns `complete=false` with the round-1 PSBT carrying the
> hot cosigner's pub nonce; once the device is back, the same
> `walletprocesspsbt psbt=... sign=true finalize=true` call drives
> both rounds and aggregates. The
> [`tests/run-musig-disconnect-scenario-speculos.sh`](tests/run-musig-disconnect-scenario-speculos.sh)
> regression simulates this with a wrapper signer script that
> short-circuits to `{"error":"device disconnected"}` while a flag
> file exists.
