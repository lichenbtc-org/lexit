# Lexit's code

Lexit is a lichen L2 (lichen, the base, at the commit `Cargo.toml` pins):
its four traits (`Front`, `Layer`, `HubL2`, `WalletL2`), implemented for
a ledger that is Liquid itself.

| crate | what |
|---|---|
| `elements` (no_std) | what the statement reads of Liquid: dynafed headers and the federation's `CHECKMULTISIG` signatures, transactions, the Merkle root, confidential commitments opened (k256); tested on mainnet block 4,153,000 and against secp256k1-zkp |
| `statement` (no_std, the guest's) | the hub's statement: a claim's grounds (malformed, not a return, too high), the terms and `D`, the journal, the return key; `request` read from Bitcoin (vlichen's) |
| `proto` | the contract's L2 part: setup terms, keys, windows, the front (vlichen's request-then-assert), `request`'s proof; Liquid as each side sees it (`liquid`: Esplora, elementsd, the mint's tree and refund, return keys and addresses, openings); a claim decided (`decide`); L-BTC held (`lbtc`: an LWK wallet, synced from an Esplora or a node); a regtest Elements chain of our own (`regtest`) |
| `hub` | `LiquidHub`, the `HubL2`: terms, the mint, the cooperative check, the dispute's front, its chores; the `lexit-hub` binary |
| `wallet` | `LiquidWallet`, the `WalletL2`: terms and mints checked, the mint swept, returns paid and claimed; the `lexit` CLI |
| `r0/` | the guest (`hp-guest`), the prover (`lexit-r0`), the build and prover scripts (`hp-split.sh`: the STARK here, the wrap elsewhere) |
| `gui/` | the wallet's GUI and app ([APPS.md](APPS.md)), its own workspace: `lexit-gui` (the page on 127.0.0.1), `lexit-devstack` (bitcoind, our Elements chain, a hub, faucets), `app/` (Tauri: Android and the desktop) |
| `deploy/` | the hub's deployment |

## Tests

```sh
LEXIT_ELEMENTSD=<elementsd 23.3> cargo test --workspace
```

The hub's tests run a regtest `bitcoind` and an Elements chain of our own
(our 2-of-3 signing its blocks, dynafed from the first): a contract
opened, its mint swept, a return paid and folded and swept; exits honest,
not a return, malformed, a lying hub refuted; an abandoned mint refunded;
an opening refused for want of L-BTC. Opt-in (`--ignored`):
`esplora_testnet` (a node-less hub on Liquid testnet's Esplora) and
`hp_real` (a dispute proved for real: `LICHEN_HP_PROVER`).

`cargo run -p lexit-proto --example hp_inputs -- <dir>` makes the
statement's inputs, one per ground, for measuring the guest
(`r0/guests/README.md`).
