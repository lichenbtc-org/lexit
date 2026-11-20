# The guest

`hp.bin` will be the program the prover (`lexit-r0`) proves and
BABE's Groth16 wrap verifies: the hub's statement (`crates/statement`,
`check`), built by `r0/build-guest.sh elf hp` (x86 Linux) then `pack hp`.
`hp.id` is its image id; a contract pins it at its opening.

`r0/Cargo.lock` and `r0/hp-guest/Cargo.lock` are committed: the builds
are `--locked`.

| guest | built | image id |
|---|---|---|
| **hp**, the statement at M4 (three grounds: malformed, not a return, too high; the federation's signatures checked, the commitments opened), RISC Zero 3.1.0, builder r0.1.97.0, 2026-11-20: **the current file** | x86 Linux (lichen-vps), `build-guest.sh elf hp` then `pack hp` there. ELF SHA-256 `092f97bfd3a0fcf39132b4e900e4470f16b376d941bf0cf7759c6339ec9e0293`; `hp.bin` 470,012 bytes, SHA-256 `53f64353634dccb8fb7775e2274bdabbdb73c012daab64a2ec52e9d055635937`. Executed on the M1 inputs: the same cycles as the native build's (below, within 33) | **`8be84dc9c422a701a5bf6bd270eb76f61b23da8424546c3342acef69e34b8ff3`** |

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
cycles, 6 segments), on a Mac (14 cores, CPU) in 346 s.

Proved for real, end to end (`crates/hub/tests/hp_real.rs`, opt-in): a
malformed claim on regtest, the hub on `hub-proves` with r0/hp-split.sh.
The STARK took 17 s on a Mac (147k user cycles, 1 segment) and the Groth16
wrap 330 s on lichen-vps (RISC Zero's prover image, Docker). The user's
wallet checked the proof under BABE, found it valid and conceded; the exit
took 365 s in all.
