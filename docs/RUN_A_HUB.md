# Running a Lexit hub

A Lexit hub sells a guaranteed way out of L-BTC. A user pays BTC into a
contract on Bitcoin and is paid L-BTC on Liquid; until the contract's end
she can return L-BTC, up to her cap, and be paid as much BTC. You hold
two floats: BTC (the contracts' liquidity, in your node's wallet) and
L-BTC (what you pay out at openings, in the hub's own wallet).

What you run: a Bitcoin node and the hub, in Docker Compose (`deploy/`).
There is no Liquid node. The hub reads Liquid from an Esplora
(Blockstream's public one by default). Every Liquid block it relies on
carries the federation's signatures, and every transaction its block's
Merkle root, so an Esplora can delay the hub but not mislead it. For
production on Liquid itself, run your own Elements node and point the hub
at it (`--elements-rpc-url`, `--elements-cookie`): see the end.

## Set up

```sh
git clone https://github.com/lichenbtc-org/lexit && cd lexit
cp deploy/.env.example deploy/.env      # NETWORK, LIQUID, your terms
deploy/build.sh                         # the images: lexit, lichen-bitcoind
deploy/lexit.sh up
deploy/lexit.sh logs hub                # its L-BTC address, once up
```

The first build compiles everything (lichen, BABE, the prover); later
builds compile only Lexit's crates (the dependencies are cached).

On first start the hub makes its keys in `deploy/state/<network>/hub/`:
the seed of its contract keys (`seed`), its Liquid key (`liquid/liquid.key`,
the key every contract's return address is derived from), and its L-BTC
wallet's mnemonic (`liquid/mnemonic`). Back that directory up: it holds
everything that can spend.

## Fund it

- **BTC**: `deploy/lexit.sh address`, then pay it. Each contract locks
  `V_max − D` of yours (the user's inbound room) and your reserve.
- **L-BTC**: `deploy/lexit.sh float` shows the address. Each opening pays
  out `D − fee`. A hub without the L-BTC to mint refuses an opening at its
  setup, before any work on it; below `FLOAT_WARN` it warns in its log.

## Your terms (`deploy/.env`)

| setting | what |
|---|---|
| `DEPOSIT_PPM_YEAR` | your yearly rate on the L-BTC you pay out (`D`): holding L-BTC, and its backing risk |
| `LIQUIDITY_PPM_YEAR` | your yearly rate on the BTC you lock beyond `D` (her inbound room) |
| `FLAT_FEE` | a flat part per contract, in sats |
| `DURATION` | a contract's life, in blocks (the network's default if empty) |
| `MAX_LIQUIDITY`, `RESERVE_PPM` | the most liquidity a contract may ask; your reserve per contract |

The fee is `flat + (D·r_D + L·r_L) · term / year`, kept out of the L-BTC
paid at the opening. Wallets compute the same fee from your `/info`.

## What it does on its own

Every 30 seconds (`HubL2::tick`):

- **Refunds abandoned mints.** A mint whose contract never opened goes
  back to your L-BTC after its timeout (3 days of Liquid blocks), by its
  refund leaf. A mint whose contract opened is the user's, never touched.
- **Sweeps returns.** Once a contract has ended (folded, or an exit paid),
  the L-BTC its user returned moves into your wallet.
- **Watches the float**, warning when it runs low.

The same on demand: `POST /liquid/refund`, `POST /liquid/sweep`;
`GET /liquid/float` reports it.

## Disputes

If a user exits on Bitcoin instead of withdrawing cooperatively, the hub
decides her claim against Liquid. A true claim is paid by its timeout. A
false one the hub proves false, with RISC Zero's proof wrapped in Groth16
(`STATEMENT=hub-proves`), and asserts within the window (on Mutinynet,
480 blocks: 4 hours). On the box itself the proof takes up to about an
hour on four cores, plus about five minutes for the wrap, which runs RISC
Zero's prover image through the host's Docker (`R0WORK`). From a Mac, or
with a GPU, `r0/hp-split.sh` proves the STARK there and wraps here.

## Your risks

- **L-BTC's backing.** You take L-BTC back at 1:1, up to the caps of your
  open contracts (`/liquid/float`'s `exposure`). Price it in your rates.
- **The federation.** A federation colluding with a user could sign a fake
  return: bounded by that contract's cap.
- **Being online.** A dispute's window is hours; an assertion missed pays
  the claim. Keep the hub up, and its prover able to finish in time.

## Production on Liquid

Run your own Elements node (`elementsd -chain=liquidv1 -validatepegin=0`,
`-txindex=1`), and start the hub with `--liquid liquid --elements-rpc-url
… --elements-cookie …`. The hub then reads and pays through your node; the
statement and the contracts are the same.
