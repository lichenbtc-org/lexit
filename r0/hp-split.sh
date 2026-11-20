#!/bin/sh
# The hub's prover, split: the STARK here, by lexit-r0 natively (a Mac's
# CPU in minutes; LEXIT_R0 names it, else r0's release build), and only
# the Groth16 wrap on an x86 Linux box with Docker (LICHEN_R0_HOST;
# LICHEN_R0_TREE its tree there, default `lexit-build`, from its home, with
# the prover image built: r0/build-guest.sh pack), where r0/hp-prover.sh
# wraps it; the seal taken from the wrap here. The hub's LICHEN_HP_PROVER,
# from a Mac.
#
#   hp-split.sh hp-proof|hp|exec-hp <hub input> <out>
set -eu
[ $# -eq 3 ] || { echo "usage: $0 hp-proof|hp|exec-hp <hub input> <out>" >&2; exit 2; }
cmd=$1 input=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
r0=${LEXIT_R0:-$here/target/release/lexit-r0}
case "$cmd" in
exec-hp) exec "$r0" exec-hp "$input" ;;
hp) exec "$r0" hp "$input" "$out" ;;
hp-proof) ;;
*) echo "usage: $0 hp-proof|hp|exec-hp <hub input> <out>" >&2; exit 2 ;;
esac
host=${LICHEN_R0_HOST:?set LICHEN_R0_HOST}
tree=${LICHEN_R0_TREE:-lexit-build}
tmp=$(mktemp -d)
dir=$(ssh "$host" mktemp -d)
trap 'rm -rf "$tmp"; ssh "$host" rm -rf "$dir"' EXIT
"$r0" hp "$input" "$tmp/receipt"
scp -q "$tmp/receipt" "$host:$dir/receipt"
ssh "$host" "$tree/r0/hp-prover.sh wrap $dir/receipt $dir/g16"
scp -q "$host:$dir/g16" "$tmp/g16"
"$r0" seal "$tmp/g16" "$out"
