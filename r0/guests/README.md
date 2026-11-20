# The guest

`hp.bin` will be the program the prover (`lexit-r0`) proves and
BABE's Groth16 wrap verifies: the hub's statement (`crates/statement`,
`check`), built by `r0/build-guest.sh elf hp` (x86 Linux) then `pack hp`.
`hp.id` is its image id; a contract pins it at its opening.

Not built yet: `hp.id` is all zeros, and only the mock statement
(`hub-proves-mock`) runs. `r0/Cargo.lock` and `r0/hp-guest/Cargo.lock`
are made at the first build (`cargo generate-lockfile`), then committed:
the builds are `--locked`.

| guest | built | image id |
|---|---|---|

## Measured (M1, 2026-11-20)

A native build of `hp-guest` on a Mac (the `risc0` toolchain, rustc
1.97; not the reproducible build, so another image id), executed by
`lexit-r0 exec-hp <input> <packed guest>` on the inputs
`cargo run -p lexit-proto --example hp_inputs -- <dir>` makes: a
synthetic `request` (regtest difficulty, ten blocks on it), and Liquid
mainnet's block 4,153,000 where a ground can use it.

| ground | input | user cycles | segments |
|---|---|---|---|
| malformed (`E` wrong) | 3.6 kB | 108k | 1 |
| not a return: no such transaction (mainnet block) | 7.0 kB | 4,841k | 5 |
| too high (mainnet header) | 6.9 kB | 4,838k | 5 |
| not a return: the output pays another script (mainnet block, its 18 kB confidential transaction) | 25.0 kB | 5,181k | 6 |
| not a return: a return, its commitments opened, the value wrong (our block: 11-of-15 test keys, mainnet's proposed params) | 7.2 kB | 4,421k | 5 |
| (calibration) the same block, another output cited: nothing opened | 7.2 kB | 3,817k | 4 |

`request`'s side costs about 0.1M cycles. A Liquid header costs about
3.7–4.7M, nearly all of it the federation's ECDSA signatures (about
0.34M a verification): `CHECKMULTISIG` tries keys in order, so mainnet's
signatures, which skip some keys, cost a few verifications more than our
eleven, which don't. Opening the commitments costs about 0.6M (the
calibration's difference); parsing the 18 kB transaction, about 0.3M.

Proved (`lexit-r0 hp`, succinct, the receipt verified): the heaviest
ground, not a return by its script on the mainnet block (5,181k user
cycles, 6 segments), on a Mac (14 cores, CPU) in 346 s. The Groth16 wrap
is still to measure (x86 Linux with Docker).
