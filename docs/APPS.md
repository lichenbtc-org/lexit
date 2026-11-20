# The Lexit wallet apps

The wallet as a page in your browser (Mac and Linux), and as an app
(Android, and the desktop), in `gui/`: glichen's GUI, adapted. The same
page and wallet either way.

- `lexit-gui` serves the page on 127.0.0.1:18350 only and opens your
  browser at it. The URL carries a token made fresh at each launch, and
  every call to the wallet must carry it, so other pages in your browser
  can't drive the wallet.
- The app (`gui/app/`, Tauri) shows the page in a webview, and its calls
  to the wallet are a command rather than HTTP.

## Screens

- **Home**: your L-BTC, and how much of it has a guaranteed exit (your
  open contracts' caps, less what's returned to them); your contracts;
  the operation running, with an opening's steps; activity.
- **Send / Receive**: plain L-BTC payments on Liquid, to and from any
  Liquid wallet. Receive shows your Liquid address with a QR code; on the
  phone, Send scans one.
- **Open a contract**: your deposit (BTC, paid to you as L-BTC less the
  fee) and extra exit room (L-BTC beyond it the contract also takes back),
  with the hub's quote. The opening shows its steps: pay the deposit (QR
  code and address; or from the wallet's own Bitcoin), garbling, the
  upload, the hub's check and its L-BTC paid on Liquid (checked there
  before the wallet signs), the funding, open. The L-BTC is then swept
  into your wallet.
- **Exit to BTC** (on a contract): one step for you. The wallet returns
  the L-BTC to the contract's return address, waits for it to confirm on
  Liquid (about a minute), and asks the hub to pay it back. The hub pays
  BTC to the contract's payout address in minutes, and the contract
  closes. If the hub doesn't pay, the contract shows **Exit on Bitcoin**:
  the same claim on chain, which the hub pays unless it proves it false.
- **Settings**: the hub (by URL; you check its key the first time, and
  the wallet is pinned to it), the chain sources (Bitcoin and Liquid
  Esploras, the Waterfalls server, or nodes), the hub's terms, your
  Bitcoin (the wallet's own payout address: exits, refunds and your
  reserve arrive there; send from it), backup.

## Trying it (testers)

Settings offers **Mutinynet · Liquid testnet**: Lexit's test hub, at
`https://thimbly.org/lichen/lexit/hub`, its key pinned in the app (a
wallet is made for it without asking you to check the key). Bitcoin is
Mutinynet (30-second blocks), Liquid is Liquid testnet; none of it is
worth anything.

1. Connect to it in Settings. "Your Bitcoin" there shows the wallet's own
   Mutinynet address: get coins from Mutinynet's faucet
   (faucet.mutinynet.com) to it.
2. Open a contract (Home). When the opening asks for the deposit, pay it
   "from your Bitcoin". Keep the app in front until it's open: your
   device garbles the dispute setup and uploads part of it to the hub (a
   few hundred MB). If it stops, "Resume the opening" on Home finishes it.
3. Your L-BTC is then in the wallet: send it, receive more (Liquid
   testnet's faucet: liquidtestnet.com/faucet), and Exit to BTC from the
   contract when you like. The BTC arrives at "Your Bitcoin".

## Build and use

```sh
cd gui
cargo build --release      # target/release/lexit-gui and lexit-devstack
target/release/lexit-gui
```

Its data is in `~/Library/Application Support/lexit-gui` (Mac) or
`~/.local/share/lexit-gui` (Linux), or `--dir`: one wallet file per
network and hub key in `wallets/`. Each holds the wallet's keys (its
contracts', and its L-BTC's mnemonic): keep the directory private, and
back it up (Settings, Backup).

The L-BTC wallet (LWK) syncs through the Liquid network's public
Waterfalls server, every 20 seconds and after each operation; Bitcoin is
read from the network's public Esplora.

## The app (Android, and the desktop)

Its identifier is `io.github.adamisz.lexit`; its wallets and settings
live in the app's own data directory.

- **Android** (arm64, sideloaded): `app/build-android.sh` builds
  `target/lexit-<version>-arm64.apk`. It needs JDK 17, the Android SDK
  (platform 35+, build-tools, an NDK), the Tauri CLI (`cargo install
  tauri-cli`) and Rust's `aarch64-linux-android` target. Install with
  `adb install -r`.
- **Desktop:** `cargo tauri build --bundles app` in `app/` (on a Mac,
  `target/release/bundle/macos/Lexit.app`, for that Mac's architecture),
  or `cargo tauri dev`. The app isn't signed by a registered developer:
  on another Mac, macOS refuses it until you allow it (right-click it,
  Open; or `xattr -dr com.apple.quarantine Lexit.app`).

An update installs over the app, keeping its wallet, only if signed with
the same key (`~/.android/debug.keystore` on the machine that built the
first APK).

## Developing: the devstack

`lexit-devstack` runs a whole deployment locally in one process: a
regtest bitcoind (`bitcoind` on the PATH, or `LICHEN_BITCOIND`), an
Elements chain of our own (`LEXIT_ELEMENTSD`, as the tests), a hub on
BABE's mock statement (no proving) with 5 L-BTC, a Bitcoin block every 3
seconds and a Liquid block every 2, and a faucet for both.

```sh
LEXIT_ELEMENTSD=<elementsd 23.3> target/release/lexit-devstack --dir /tmp/lexit-devstack
target/release/lexit-gui --devstack /tmp/lexit-devstack/devstack.json
```

An opening then offers "Pay it from the devstack's faucet", and Receive
"Get 100,000 sats of L-BTC". `--refuse-folds` makes the hub refuse
cooperative exits, to see the dispute path.
