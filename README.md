# Lexit

A [lichen](https://github.com/lichenbtc-org/lichen) L2 whose ledger is
Liquid: a hub swaps you into L-BTC, and a lichen contract on Bitcoin
guarantees your way back out to BTC, up to your cap, until the contract's
end. No swap provider or peg-out whitelist is involved. Payments are plain
L-BTC; the hub only mints at an opening and takes returns at an exit.

An early skeleton: it builds, and its tests pass on real Liquid data; it
hasn't run end to end.

```sh
cargo test --workspace
```

| crate | what |
|---|---|
| `crates/elements` | what the statement reads of Liquid (no_std): dynafed headers and the federation's signatures, transactions, the Merkle root, confidential commitments opened |
| `crates/statement` | the hub's statement (no_std, the guest's): an exit claim citing returns of L-BTC is false |
| `crates/proto` | the contract's L2 part: terms, keys, the front, Liquid views, mint and return keys |
| `crates/hub` | the hub (`lexit-hub`) |
| `crates/wallet` | the wallet (`lexit`) |
| `r0/` | the RISC Zero guest and its prover (not built yet) |
