#!/bin/sh
# r0/hp-prover.sh on another machine, over ssh (an x86 Linux box with
# Docker, for the Groth16 wrap): LICHEN_R0_HOST names it, LICHEN_R0_TREE
# Lexit's tree there (default `lexit`, from its home), with the prover
# image built (r0/build-guest.sh pack, or docker build -f r0/Dockerfile -t
# lexit-r0 .). The hub's LICHEN_HP_PROVER, from a Mac.
#
#   hp-remote.sh hp-proof|hp|exec-hp <hub input> <out>
set -eu
[ $# -eq 3 ] || { echo "usage: $0 hp-proof|hp|exec-hp <hub input> <out>" >&2; exit 2; }
cmd=$1 input=$2 out=$3
host=${LICHEN_R0_HOST:?set LICHEN_R0_HOST}
tree=${LICHEN_R0_TREE:-lexit}
dir=$(ssh "$host" mktemp -d)
trap 'ssh "$host" rm -rf "$dir"' EXIT
scp -q "$input" "$host:$dir/input.bin"
ssh "$host" "$tree/r0/hp-prover.sh $cmd $dir/input.bin $dir/out"
[ "$cmd" = exec-hp ] || scp -q "$host:$dir/out" "$out"
