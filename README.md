# Lexit

L-BTC with a guaranteed exit. A [lichen](https://github.com/lichenbtc-org/lichen)
L2 whose ledger is Liquid itself: a hub swaps you into L-BTC, and a lichen
contract on Bitcoin guarantees your way back out to BTC, up to your cap,
until the contract's end. No swap provider or peg-out whitelist is
involved, and you don't have to trust the hub to honour it.

## Boilerplate warning:

Under no circumstances should you use this with real money yet!
Code is LLM generated and needs thorough auditing, first.

## How it works

- **Opening** is one atomic swap. You pay `D` BTC into a contract on
  Bitcoin. The hub pays you `D − fee` L-BTC on Liquid, locked to a key you
  can use only once your funding is on Bitcoin; your wallet checks it on
  Liquid before you sign. Your cap is `D − fee` plus the inbound liquidity
  you asked for.
- **Payments** are plain L-BTC, in any Liquid wallet. The hub isn't
  involved.
- **Exiting**: you pay L-BTC back to your contract's return address on
  Liquid (a fresh key, linked to nothing), and the hub pays you as much
  BTC from the contract, cooperatively, in minutes. If it won't, you claim
  on Bitcoin: the hub must prove your claim false, with a RISC Zero proof
  checked by BABE, or its timeout pays you.
- **The hub** runs a Bitcoin node and reads Liquid from an Esplora (or its
  own Elements node). It charges a yearly rate on both legs it commits:
  the L-BTC it pays out and the BTC it locks beyond your deposit, for the
  contract's term. It carries L-BTC's backing risk, up to its caps.

You trust Liquid's federation for liveness and censorship, as every L-BTC
holder does, not for custody up to your cap. If Liquid stops making
blocks, you can't exit until it restarts.

## Status

Working end to end on test networks, as a command-line wallet and a
Docker-deployed hub:

- the statement, built reproducibly (image id `8be84dc9…`), 3.8–5.2M cycles
  a ground; a dispute proved for real on regtest (the STARK on a Mac, the
  Groth16 wrap on x86);
- a hub on Mutinynet and Liquid testnet with no Liquid node, and a first
  contract there: opened, the mint swept, L-BTC returned, withdrawn
  cooperatively;
- the hub's chores: abandoned openings' mints refunded after their
  timeout, returns swept, its float watched.

Still to come: wallet apps (desktop and Android), a public hub endpoint,
and the work before mainnet (an audit, RISC Zero's Groth16 setup with a
contribution of our own).

## Docs

- [docs/USE_A_WALLET.md](docs/USE_A_WALLET.md): the `lexit` wallet
- [docs/RUN_A_HUB.md](docs/RUN_A_HUB.md): running a hub (`deploy/`)
- [docs/CODE.md](docs/CODE.md): the crates and the tests

## Build and test

```sh
cargo build --release              # target/release/lexit, lexit-hub
LEXIT_ELEMENTSD=<elementsd 23.3> cargo test --workspace
```

The tests run a regtest `bitcoind` (on the PATH, or `LICHEN_BITCOIND`)
and an Elements regtest chain of their own (our 2-of-3 signing its
blocks), about two minutes. Opt-in (`--ignored`): `esplora_testnet` (a
hub on Liquid testnet's real Esplora) and `hp_real` (a dispute proved for
real; `LICHEN_HP_PROVER`).

| crate | what |
|---|---|
| `crates/elements` | what the statement reads of Liquid (no_std): dynafed headers and the federation's signatures, transactions, the Merkle root, confidential commitments opened |
| `crates/statement` | the hub's statement (no_std, the guest's): an exit claim citing returns of L-BTC is false |
| `crates/proto` | the contract's L2 part: terms, keys, the front, Liquid views, the mint and return keys, the L-BTC wallet (LWK), a regtest Elements chain |
| `crates/hub` | the hub (`lexit-hub`) |
| `crates/wallet` | the wallet (`lexit`) |
| `r0/` | the RISC Zero guest and its prover |
| `deploy/` | the hub's deployment |
