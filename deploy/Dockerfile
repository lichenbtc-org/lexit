# The operator's image, for the hub (deploy/docker-compose.yml): the hub
# (`lexit-hub`), the wallet (`lexit`, for trials from the box), and the
# prover `lexit-r0` (RISC Zero 3.1.0, in process) with the committed guest
# it reads. A Groth16 wrap runs RISC Zero's prover image through the host's
# Docker (its socket mounted). Build context: a directory holding lexit/
# (deploy/build.sh makes it); lichen and lichen-btc come by git.
#
# The dependencies are built in a layer of their own (cargo-chef): a
# rebuild after a change to Lexit's crates compiles only those.
FROM rust:1.96-bookworm AS chef
RUN cargo install cargo-chef --locked --version 0.1.78
RUN apt-get update && apt-get install -y --no-install-recommends clang && rm -rf /var/lib/apt/lists/*
WORKDIR /src/lexit

FROM chef AS plan
COPY lexit .
RUN cargo chef prepare --recipe-path /recipe.json

FROM chef AS build
COPY --from=plan /recipe.json /recipe.json
RUN cargo chef cook --release --recipe-path /recipe.json -p lexit-hub -p lexit-wallet
COPY lexit .
# the commit the hub reports in its offer (`GET /info`)
ARG LICHEN_COMMIT=
RUN cargo build --release -p lexit-hub -p lexit-wallet
RUN mkdir -p /out && cp target/release/lexit-hub target/release/lexit /out/

# the prover: Lexit's RISC Zero workspace (the guest is committed, so
# building the host needs no guest toolchain); a layer of its own, rebuilt
# only when the statement or r0/ change
FROM chef AS prover
COPY lexit/crates/elements crates/elements
COPY lexit/crates/statement crates/statement
COPY lexit/r0 r0
RUN cargo build --release --locked --manifest-path r0/Cargo.toml -p lexit-r0 && r0/target/release/lexit-r0 check-id

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
# the docker CLI for the wrap (static; bookworm's docker.io speaks an API
# older than a current daemon's minimum)
COPY --from=docker:28.5.1-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=build /out/lexit-hub /out/lexit /usr/local/bin/
COPY --from=prover /src/lexit/r0/target/release/lexit-r0 /usr/local/bin/
# lexit-r0 reads the committed guest from its source tree's path,
# r0/host/../guests: r0/host has to exist for that to resolve
COPY --from=prover /src/lexit/r0/guests /src/lexit/r0/guests
RUN mkdir -p /src/lexit/r0/host
COPY lexit/r0/hp-prover.sh /usr/local/bin/lexit-hp-prover
COPY lexit/deploy/entry.sh /usr/local/bin/lexit-entry
ENTRYPOINT ["/usr/local/bin/lexit-entry"]
