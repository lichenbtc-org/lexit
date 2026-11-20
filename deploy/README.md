# Deploying Lexit

One deployment per Bitcoin network, run by the hub's operator: the
Bitcoin node and the hub, in one Docker Compose project
([docs/RUN_A_HUB.md](../docs/RUN_A_HUB.md)). There is no Liquid node: the
hub reads Liquid from an Esplora and holds its L-BTC in a wallet of its
own. Users run only the `lexit` wallet, with no node
([docs/USE_A_WALLET.md](../docs/USE_A_WALLET.md)).

| path | what |
|---|---|
| `.env.example` | the settings; copy to `.env` |
| `docker-compose.yml` | the deployment: `bitcoind`, `hub`, and on regtest `miner` |
| `Dockerfile`, `entry.sh` | the `lexit` image (the hub, `lexit-r0`, the wallet) and its roles: `hub`, `float` |
| `bitcoind/` | the node's image (`lichen-bitcoind:<network>`, vlichen's) |
| `build.sh` | builds both images |
| `context.sh` | a build context of committed sources (Lexit's HEAD), for `build.sh` |
| `lexit.sh` | the operator's commands: `up`, `down`, `info`, `float`, `address`, `balance`, `fund`, `logs`, `cli` |
| `state/<network>/` | everything kept: `bitcoind/`, `hub/` (not in git) |
