# Sourced by deploy/build.sh: a Docker build context of committed sources
# only, lexit's HEAD exported with `git archive` (no working-tree
# changes, no target/). lichen and lichen-btc come by git, at the commits
# Cargo.lock pins.
#   make_context      sets $ctx: a directory holding lexit/ (removed on
#                     exit), and $lichen_commit: lexit's HEAD as `git
#                     describe` names it (its tag, if any)

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

make_context() {
    local deploy lexit
    deploy="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    lexit="$(cd "$deploy/.." && pwd)"
    if [ -n "$(git -C "$lexit" status --porcelain -- crates Cargo.toml Cargo.lock deploy)" ]; then
        echo "note: uncommitted changes in lexit are not in the image (it builds HEAD)" >&2
    fi
    ctx="$(mktemp -d)"
    trap 'rm -rf "$ctx"' EXIT
    mkdir -p "$ctx/lexit"
    git -C "$lexit" archive HEAD | tar -x -C "$ctx/lexit"
    lichen_commit="$(git -C "$lexit" describe --tags --always)"
    echo "== lexit $lichen_commit"
}
