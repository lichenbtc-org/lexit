#!/usr/bin/env bash
# The operator's commands (deploy/.env, or ENV_FILE):
#   deploy/lexit.sh up                  start the deployment (or bring it up to date after a build)
#   deploy/lexit.sh down                stop it
#   deploy/lexit.sh info                the hub's offer (`GET /info`) and the node's chain
#   deploy/lexit.sh float               the hub's L-BTC, the address to fund it at, its mints outstanding, its exposure
#   deploy/lexit.sh address             an address of the node's wallet `hub` (its BTC: the contracts' funding)
#   deploy/lexit.sh balance             that wallet's balance
#   deploy/lexit.sh fund <btc>          regtest: pay that wallet from the miner's coins
#   deploy/lexit.sh logs [service]      follow the logs (bitcoind, hub)
#   deploy/lexit.sh cli <args>          bitcoin-cli on the node
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
env_file="${ENV_FILE:-$here/.env}"
# shellcheck disable=SC1090
. "$env_file"
data="$here/state/$NETWORK"
# the wrap's working files, at the same path on the host and in the hub
export R0WORK="${R0WORK:-$data/r0work}"
dc() { docker compose --env-file "$env_file" -f "$here/docker-compose.yml" "$@"; }
cli() { dc exec -T bitcoind bitcoin-cli -datadir=/data "$@"; }
hub_url="http://127.0.0.1:${HUB_PORT:-18330}"

case "${1:-}" in
    up)
        mkdir -p "$data/bitcoind" "$data/hub" "$R0WORK"
        dc up -d
        ;;
    down) dc down ;;
    info)
        echo "hub:     $(curl -fsS "$hub_url/info" 2>/dev/null || echo '(no answer yet: deploy/lexit.sh logs hub)')"
        cli getblockchaininfo | grep -E '"(chain|blocks|headers|initialblockdownload)"'
        ;;
    float) curl -fsS "$hub_url/liquid/float"; echo ;;
    address) cli -rpcwallet=hub getnewaddress ;;
    balance) cli -rpcwallet=hub getbalance ;;
    fund)
        addr="$(cli -rpcwallet=hub getnewaddress)"
        cli -rpcwallet=miner sendtoaddress "$addr" "${2:?amount in BTC}"
        ;;
    logs) shift; dc logs -f "$@" ;;
    cli) shift; cli "$@" ;;
    *) sed -n '2,11p' "$0"; exit 1 ;;
esac
