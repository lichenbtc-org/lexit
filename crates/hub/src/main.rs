//! `lexit-hub`: the hub daemon (a lichen hub selling a guaranteed
//! exit from L-BTC). It runs beside a Bitcoin node (its wallet funds the
//! contracts), and reads Liquid from an Esplora (the network's public one
//! by default) or from an Elements node of its own; its L-BTC is an LWK
//! wallet of its own.
//!
//! Its secrets are made on first start and kept in `--dir`: the seed of
//! its contract keys (`seed`), its Liquid key (`liquid/liquid.key`, the
//! returns' key) and its L-BTC wallet's mnemonic (`liquid/mnemonic`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use lichen_proto::{fresh_rho, Statement};
use lexit_hub::{liquid_dir, Config, Hub, LiquidHub, LiquidSource};
use lexit_proto::liquid::Rpc;
use lexit_proto::LiquidNet;

#[derive(Parser)]
#[command(about = "A Lexit hub")]
struct Args {
    /// The Bitcoin network: `regtest`, `mutinynet`, `signet` or `testnet4`.
    #[arg(long, default_value = "regtest")]
    network: lichen_proto::Net,
    /// The Liquid network: `liquid`, `testnet` or `regtest`.
    #[arg(long, default_value = "regtest")]
    liquid: String,
    #[arg(long, default_value = "127.0.0.1:18330")]
    listen: String,
    /// The Bitcoin node's RPC URL and cookie file; the hub's wallet there.
    #[arg(long)]
    rpc_url: Option<String>,
    #[arg(long)]
    cookie: Option<PathBuf>,
    #[arg(long, default_value = "hub")]
    chain_wallet: String,
    /// Liquid's Esplora (the network's public one by default).
    #[arg(long)]
    liquid_esplora: Option<String>,
    /// Instead of an Esplora: an Elements node's RPC URL and cookie file
    /// (a production hub's own node; a regtest chain's).
    #[arg(long)]
    elements_rpc_url: Option<String>,
    #[arg(long)]
    elements_cookie: Option<PathBuf>,
    /// Warn when the hub's L-BTC falls below this, in sats.
    #[arg(long, default_value_t = 1_000_000)]
    float_warn: u64,
    /// Seconds between the hub's chores (refunds, sweeps, the float), and
    /// the most its L-BTC wallet may lag Liquid: a sync is a full scan, and a
    /// public Esplora rate-limits.
    #[arg(long, default_value_t = 300)]
    chores_every: u64,
    #[arg(long, default_value_t = 60)]
    sync_every: u64,
    /// The hub's statement: `hub-proves-mock` (a proof of a toy circuit,
    /// for trials) or `hub-proves` (its guest, not built yet).
    #[arg(long, default_value = "hub-proves-mock")]
    statement: String,
    /// Where the hub keeps its contracts and keys.
    #[arg(long, default_value = "hub-data")]
    dir: PathBuf,
    /// The hub's reserve, parts per million of the cap.
    #[arg(long, default_value_t = 20_000)]
    reserve_ppm: u64,
    /// The hub's fee, kept out of the L-BTC it pays the user at the
    /// opening: a yearly rate, in parts per million, on that L-BTC (`D`:
    /// its cost of holding it and its risk on it) ...
    #[arg(long, default_value_t = 30_000)]
    deposit_ppm_year: u64,
    /// ... and on the BTC it locks in the contract beyond `D` (the
    /// liquidity), for the contract's term ...
    #[arg(long, default_value_t = 50_000)]
    liquidity_ppm_year: u64,
    /// ... and a flat part per contract, in sats.
    #[arg(long, default_value_t = 0)]
    flat_fee: u64,
    /// The most liquidity a contract may ask, in sats.
    #[arg(long, default_value_t = 10_000_000)]
    max_liquidity: u64,
    /// Blocks from opening to T_close (the network's default).
    #[arg(long)]
    duration: Option<u32>,
    /// For trials, on test networks only: refuse every cooperative
    /// withdrawal and assert a false proof against every exit claim.
    #[arg(long)]
    hostile: bool,
    /// For trials, on test networks only: refuse every cooperative
    /// withdrawal, asserting only against false claims.
    #[arg(long)]
    refuse_folds: bool,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let liquid: LiquidNet = serde_json::from_value(serde_json::Value::String(a.liquid.clone())).context("--liquid: `liquid`, `testnet` or `regtest`")?;
    let seed_file = a.dir.join("seed");
    if !seed_file.exists() {
        std::fs::create_dir_all(&a.dir)?;
        std::fs::write(&seed_file, hex::encode(fresh_rho()))?;
    }
    let seed = std::fs::read_to_string(&seed_file)?.trim().to_string();
    if a.hostile || a.refuse_folds {
        anyhow::ensure!(a.network.is_test(), "--hostile and --refuse-folds are for test networks only, not {}", a.network);
    }
    let source = match (a.elements_rpc_url.clone(), a.elements_cookie.clone()) {
        (Some(url), Some(cookie)) => LiquidSource::Node(Rpc { url, cookie, wallet: String::new() }),
        (None, None) => LiquidSource::Esplora(a.liquid_esplora.clone().or(liquid.default_esplora().map(str::to_string)).context("--liquid-esplora: no public Esplora for this Liquid network")?),
        _ => anyhow::bail!("--elements-rpc-url and --elements-cookie go together"),
    };
    let pricing = lichen_proto::Pricing { deposit_ppm_year: a.deposit_ppm_year, liquidity_ppm_year: a.liquidity_ppm_year, flat: a.flat_fee };
    let mut l2 = LiquidHub::open(&liquid_dir(&a.dir), liquid, source, pricing)?;
    l2.float_warn = a.float_warn;
    l2.chores_every = std::time::Duration::from_secs(a.chores_every);
    l2.sync_every = std::time::Duration::from_secs(a.sync_every);
    println!("the hub's L-BTC: {} sats; fund it at {}", l2.balance()?, l2.address()?);
    let hub = Hub::start(
        Config {
            network: a.network,
            listen: a.listen,
            rpc_url: a.rpc_url.context("--rpc-url: the Bitcoin node's RPC URL")?,
            cookie: a.cookie.context("--cookie: the Bitcoin node's cookie file")?,
            wallet: a.chain_wallet,
            statement: Statement::parse(&a.statement, lichen_babe::statement::image_id(lexit_proto::HP_IMAGE_ID))?,
            prover: Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../r0/hp-prover.sh"))),
            dir: a.dir.clone(),
            seed,
            reserve_ppm: a.reserve_ppm,
            // unused: the fee is the pricing's
            fee_ppm: 0,
            max_liquidity: a.max_liquidity,
            duration: a.duration.unwrap_or(lichen_proto::default_duration(a.network)),
            refuse_folds: a.hostile || a.refuse_folds,
            forge_assertions: a.hostile,
            first_cid: 1000,
        },
        l2,
    )?;
    println!("Lexit hub at {}", hub.url);
    loop {
        std::thread::park();
    }
}
