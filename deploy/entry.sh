#!/bin/sh
# The image's roles (deploy/docker-compose.yml):
#   hub   the hub, on NETWORK's node (its cookie in /bitcoind), reading
#         LIQUID from LIQUID_ESPLORA (its public Esplora by default); its
#         keys and L-BTC wallet in /hub. On the real statement
#         (STATEMENT=hub-proves) it proves with lexit-r0 (r0/hp-prover.sh),
#         wrapping in Groth16 through the host's Docker
#   float the hub's float: its L-BTC, the address to fund it at, its mints
#         outstanding, its exposure
set -eu
NETWORK="${NETWORK:?set NETWORK}"
case "$NETWORK" in
    regtest) chaindir=regtest ;;
    mutinynet | signet) chaindir=signet ;;
    testnet4) chaindir=testnet4 ;;
    *) echo "unknown NETWORK $NETWORK" >&2; exit 1 ;;
esac
cookie="/bitcoind/$chaindir/.cookie"
node=http://bitcoind:8332

case "${1:-}" in
    hub)
        while [ ! -f "$cookie" ]; do echo "waiting for $cookie"; sleep 2; done
        export LICHEN_R0=/usr/local/bin/lexit-r0 LICHEN_HP_PROVER=/usr/local/bin/lexit-hp-prover
        exec lexit-hub --network "$NETWORK" --liquid "${LIQUID:-testnet}" ${LIQUID_ESPLORA:+--liquid-esplora "$LIQUID_ESPLORA"} \
            --listen 0.0.0.0:18330 --rpc-url "$node" --cookie "$cookie" --chain-wallet hub --dir /hub \
            --statement "${STATEMENT:-hub-proves-mock}" --reserve-ppm "${RESERVE_PPM:-20000}" \
            --deposit-ppm-year "${DEPOSIT_PPM_YEAR:-30000}" --liquidity-ppm-year "${LIQUIDITY_PPM_YEAR:-50000}" --flat-fee "${FLAT_FEE:-0}" \
            --max-liquidity "${MAX_LIQUIDITY:-10000000}" --float-warn "${FLOAT_WARN:-1000000}" \
            ${DURATION:+--duration "$DURATION"} ${HOSTILE:+--hostile} ${REFUSE_FOLDS:+--refuse-folds}
        ;;
    float)
        exec curl -fsS http://hub:18330/liquid/float
        ;;
    *)
        exec "$@"
        ;;
esac
