#!/usr/bin/env bash
# A smoke test of a running deployment, from the box: a user's wallet, as
# one-off containers on the deployment's compose network, its BTC a wallet
# `smoke` of the deployment's node, its L-BTC read from LIQUID_ESPLORA.
#   deploy/smoke.sh fund               an address to pay the smoke wallet's BTC at (the node's wallet `smoke`)
#   deploy/smoke.sh open [D] [L]       open a contract (D sats in, L of liquidity), the deposit paid from it
#   deploy/smoke.sh status             its contracts and its L-BTC
#   deploy/smoke.sh return <c> <sats>  return L-BTC for contract c
#   deploy/smoke.sh withdraw <c> <sats>  withdraw it cooperatively (once the return confirms)
#   deploy/smoke.sh lexit <args>       any wallet command
# (deploy/.env, or ENV_FILE; the wallet in deploy/state/<network>/smoke/)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
env_file="${ENV_FILE:-$here/.env}"
# shellcheck disable=SC1090
. "$env_file"
data="$here/state/$NETWORK"
export R0WORK="${R0WORK:-$data/r0work}"
case "$NETWORK" in regtest) chaindir=regtest ;; mutinynet | signet) chaindir=signet ;; testnet4) chaindir=testnet4 ;; esac
dc() { docker compose --env-file "$env_file" -f "$here/docker-compose.yml" "$@"; }
mkdir -p "$data/smoke"
lexit() {
    dc run --rm --no-deps -T -v "$data/smoke:/w" -v "$data/bitcoind:/bitcoind:ro" --entrypoint lexit hub \
        --network "$NETWORK" --wallet /w/smoke.json --hub http://hub:18330 \
        --rpc-url http://bitcoind:8332 --cookie "/bitcoind/$chaindir/.cookie" --chain-wallet smoke \
        ${LIQUID_ESPLORA:+--esplora-liquid "$LIQUID_ESPLORA"} --statement "${STATEMENT:-hub-proves-mock}" "$@"
}
[ -f "$data/smoke/smoke.json" ] || lexit init
case "${1:-}" in
    fund)
        cli() { dc exec -T bitcoind bitcoin-cli -datadir=/data "$@"; }
        cli createwallet smoke >/dev/null 2>&1 || cli loadwallet smoke >/dev/null 2>&1 || true
        echo "pay the smoke wallet's BTC at $(cli -rpcwallet=smoke getnewaddress); it has $(cli -rpcwallet=smoke getbalance) BTC"
        ;;
    open) lexit open "${2:-50000}" --liquidity "${3:-100000}" --reserve 10000 --pay ;;
    status) lexit contracts; lexit balance-lbtc ;;
    return) lexit return "${2:?contract}" "${3:?sats}" ;;
    withdraw) lexit withdraw "${2:?contract}" "${3:?sats}" ;;
    lexit) shift; lexit "$@" ;;
    *) sed -n '2,11p' "$0"; exit 1 ;;
esac
