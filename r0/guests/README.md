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
