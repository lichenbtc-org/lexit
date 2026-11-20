# Using the Lexit wallet

`lexit` is the user's wallet as a command line (the same wallet as a page
in your browser, and as an app for Android and the desktop:
[APPS.md](APPS.md)).
With it you swap BTC into L-BTC through a hub, and hold a contract that
guarantees your way back: until the contract's end, L-BTC you return to it
is paid back in BTC, up to your cap, whatever the hub does. You run no
node: it reads Bitcoin and Liquid from public Esploras.

## Make a wallet

```sh
lexit --network mutinynet --hub https://<hub> init
```

The wallet file (`wallet.json`, or `--wallet`) holds your keys: its
contracts' keys and your L-BTC wallet's mnemonic. Keep it private and
backed up.

## Open a contract

```sh
lexit --network mutinynet --hub https://<hub> open 200000 --liquidity 1000000
```

You pay `200000` sats of BTC in (the command tells you where, and waits);
the hub pays you L-BTC, less its fee, on Liquid. Your cap is the deposit
less the fee plus the liquidity you asked for: how much L-BTC, in all,
the contract will take back. The hub's fee depends on both amounts and
the contract's term (`/info`).

The swap is atomic: the hub's L-BTC is locked to your key until your
funding is on Bitcoin, and your wallet checks it on Liquid before it
signs. Once the funding is out, the wallet sweeps it into your L-BTC.

## Your L-BTC

```sh
lexit balance-lbtc
lexit address-lbtc            # receive L-BTC
lexit send-lbtc <address> <sats>
```

## Get your BTC back

```sh
lexit return <contract> 150000     # pay L-BTC back to the contract
lexit withdraw <contract> 150000   # once it confirms: the hub pays you BTC
```

`return` pays to your contract's return address on Liquid (a fresh key,
linked to nothing) and records the payment for your claim. `withdraw`
asks the hub to pay you cooperatively, in minutes.

If the hub won't, `lexit exit <contract>` claims your returns on Bitcoin.
The hub must prove the claim false or its timeout pays you; the wallet
follows the dispute to the end, so leave it running (or run it again: it
picks up where it was). It takes hours, and Bitcoin fees from the budget
your deposit carried.

## Before the contract's end

`lexit contracts` shows each contract's end (`T_close`). After it the
hub takes the contract back, and what you hadn't withdrawn stays L-BTC,
unguaranteed. Withdraw before it, and open a new contract if you want the
guarantee to go on.

## Things to know

- **A Liquid halt.** If Liquid stops making blocks you can't return L-BTC,
  so you can't exit until it restarts.
- **An abandoned opening.** If an opening stalls after you paid your
  deposit, refund it (`lexit refund <contract>`) once its timeout allows,
  well within a day: don't leave it.
- **Returns from another wallet.** You can pay a return from any Liquid
  wallet, then record it with its blinding factors (`lexit record-return`).
