//! A local stack for developing the GUI: a regtest bitcoind, an Elements
//! chain of our own (our 2-of-3 signing its blocks), a hub on BABE's mock
//! statement (every claim true, so a dispute needs no proving), blocks on
//! both chains every few seconds, and a faucet paying BTC and L-BTC. Each
//! run starts fresh: a new hub, so a GUI wallet pinned to an earlier run's
//! is never shown another.
//!
//! It needs `bitcoind` (`LICHEN_BITCOIND`) and `elementsd`
//! (`LEXIT_ELEMENTSD`), as Lexit's tests do. It writes
//! `<dir>/devstack.json`, the GUI's connection for it:
//! `lexit-gui --devstack <dir>/devstack.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use bitcoin::Amount;
use bitcoincore_rpc::{Auth, RpcApi};
use clap::Parser;
use lexit_hub::{Config, Hub, LiquidHub, LiquidSource};
use lexit_proto::liquid::{LiquidNet, Rpc};
use lexit_proto::regtest::{mine_with, ElementsRegtest, Federation};
use lichen_btc::regtest::Regtest;
use lichen_chain::{Chain, Net};
use lichen_proto::Statement;
use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

#[derive(Parser)]
#[command(about = "A local Lexit stack for developing the GUI")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:28350")]
    hub: String,
    /// The faucet: `POST /pay {"address", "sats"}` (BTC), `POST /pay-lbtc
    /// {"address", "sats"}`, `POST /mine {"blocks"}`, `POST /address` (a
    /// new address of the Bitcoin node's wallet).
    #[arg(long, default_value = "127.0.0.1:28359")]
    faucet: String,
    /// Seconds between Bitcoin blocks.
    #[arg(long, default_value_t = 3)]
    mine_every: u64,
    /// Seconds between Liquid blocks.
    #[arg(long, default_value_t = 2)]
    liquid_every: u64,
    /// Wiped at each start.
    #[arg(long, default_value = "devstack")]
    dir: PathBuf,
    /// The hub refuses cooperative withdrawals (the GUI's dispute path).
    #[arg(long)]
    refuse_folds: bool,
}

/// An amount of sats as a decimal of BTC, as the node's RPC takes it.
fn btc(sats: u64) -> String {
    format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let faucet = Server::http(&args.faucet).map_err(|e| anyhow::anyhow!("listen on {}: {e}", args.faucet))?;
    // only a directory a devstack made is wiped
    let marker = args.dir.join(".lexit-devstack");
    if args.dir.exists() && std::fs::read_dir(&args.dir)?.next().is_some() && !marker.exists() {
        bail!("{} has files that aren't a devstack's, and the devstack wipes its directory: name another --dir", args.dir.display());
    }
    let _ = std::fs::remove_dir_all(&args.dir);
    std::fs::create_dir_all(&args.dir)?;
    std::fs::write(&marker, "")?;
    let dir = std::fs::canonicalize(&args.dir)?;

    println!("starting bitcoind (regtest), elementsd (our own chain) and the hub ...");
    let rt = Arc::new(Regtest::start()?);
    let el = ElementsRegtest::start(Federation::regtest()).context("elementsd (LEXIT_ELEMENTSD)")?;
    let (el_rpc, el_dir) = (el.rpc.clone(), el.datadir.clone());

    // blocks on both chains, in the background
    let miner = rt.clone();
    let every = Duration::from_secs(args.mine_every);
    std::thread::spawn(move || loop {
        let _ = miner.mine(1);
        std::thread::sleep(every);
    });
    let (rpc, fed) = (el.rpc.clone(), el.federation.clone());
    let every = Duration::from_secs(args.liquid_every);
    std::thread::spawn(move || loop {
        let _ = mine_with(&rpc, &fed, 1);
        std::thread::sleep(every);
    });

    // the hub: BTC in its node wallet (several coins, so it can fund more
    // than one contract a block), L-BTC in its own wallet, read through
    // the Elements node
    let hub_chain = Chain::connect(rt.rpc_url(), rt.cookie(), "hub")?;
    for _ in 0..8 {
        rt.fund(&hub_chain.new_spk()?, Amount::from_sat(50_000_000))?;
    }
    let node = Rpc { wallet: String::new(), ..el.rpc.clone() };
    let pricing = lichen_proto::Pricing { deposit_ppm_year: 20_000, liquidity_ppm_year: 50_000, flat: 100 };
    let mut l2 = LiquidHub::open(&dir.join("hub/liquid"), LiquidNet::Regtest, LiquidSource::Node(node), pricing)?;
    l2.chores_every = Duration::from_secs(5);
    l2.sync_every = Duration::ZERO;
    let lbtc = 5 * 100_000_000;
    el.rpc.wallet_call("sendtoaddress", json!([l2.address()?.to_string(), btc(lbtc)]))?;
    let start = Instant::now();
    while l2.balance()? < lbtc {
        anyhow::ensure!(start.elapsed() < Duration::from_secs(60), "the hub's L-BTC didn't confirm");
        std::thread::sleep(Duration::from_millis(500));
    }
    let hub = Hub::start(
        Config {
            network: Net::Regtest,
            listen: args.hub.clone(),
            rpc_url: rt.rpc_url().to_string(),
            cookie: rt.cookie().clone(),
            wallet: "hub".into(),
            statement: Statement::parse("hub-proves-mock", lichen_babe::statement::image_id(lexit_proto::HP_IMAGE_ID))?,
            prover: None,
            dir: dir.join("hub"),
            seed: format!("lexit-devstack-hub-{}", std::process::id()),
            reserve_ppm: 20_000,
            fee_ppm: 0,
            max_liquidity: 20_000_000,
            duration: 2_000,
            refuse_folds: args.refuse_folds,
            forge_assertions: false,
            first_cid: 1000,
        },
        l2,
    )?;
    let conn = json!({
        "network": "regtest",
        "hub": hub.url,
        "rpc_url": rt.rpc_url(),
        "cookie": rt.cookie(),
        "elements_rpc_url": el.rpc.url,
        "elements_cookie": el.rpc.cookie,
        "faucet": format!("http://{}", args.faucet),
    });
    // written whole, and only once everything answers: a GUI waits for it
    let file = dir.join("devstack.json");
    std::fs::write(dir.join("devstack.json.tmp"), serde_json::to_vec_pretty(&conn)?)?;
    std::fs::rename(dir.join("devstack.json.tmp"), &file)?;

    // Ctrl-C: both nodes stopped and their data removed
    let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM])?;
    let stop = rt.clone();
    let stale = file.clone();
    let el_stop = el_rpc.clone();
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            println!("stopping ...");
            // no GUI started later is pointed at nodes that are gone
            let _ = std::fs::remove_file(&stale);
            let _ = el_stop.call("stop", json!([]));
            if let Ok(rpc) = bitcoincore_rpc::Client::new(stop.rpc_url(), Auth::CookieFile(stop.cookie().clone())) {
                let _ = rpc.stop();
                let start = Instant::now();
                while (rpc.get_block_count().is_ok() || el_stop.call("getblockcount", json!([])).is_ok()) && start.elapsed() < Duration::from_secs(10) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = std::fs::remove_dir_all(stop.datadir());
            let _ = std::fs::remove_dir_all(&el_dir);
            std::process::exit(130);
        }
    });

    println!("the devstack: hub {}, faucet http://{}", hub.url, args.faucet);
    println!("the GUI's connection: lexit-gui --devstack {}", file.display());
    for mut req in faucet.incoming_requests() {
        let mut body = String::new();
        let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
        let (code, text) = match route(&rt, &el_rpc, req.method(), req.url(), &body) {
            Ok(v) => (200, v.to_string()),
            Err(e) => (400, json!({ "error": format!("{e:#}") }).to_string()),
        };
        let h = Header::from_bytes("Content-Type", "application/json").unwrap();
        let _ = req.respond(Response::from_string(text).with_status_code(code).with_header(h));
    }
    drop(hub);
    drop(el);
    Ok(())
}

fn route(rt: &Regtest, el: &Rpc, method: &Method, url: &str, body: &str) -> Result<serde_json::Value> {
    let v: serde_json::Value = if body.trim().is_empty() { json!({}) } else { serde_json::from_str(body)? };
    match (method, url) {
        (Method::Post, "/pay") => {
            let addr: bitcoin::Address<bitcoin::address::NetworkUnchecked> = v["address"].as_str().context("address")?.parse()?;
            let addr = addr.require_network(bitcoin::Network::Regtest)?;
            let sats = v["sats"].as_u64().context("sats")?;
            let (op, _) = rt.fund(&addr.script_pubkey(), Amount::from_sat(sats))?;
            println!("faucet: paid {sats} sats to {addr} ({op})");
            Ok(json!({ "outpoint": op.to_string() }))
        }
        (Method::Post, "/pay-lbtc") => {
            let addr = v["address"].as_str().context("address")?;
            let sats = v["sats"].as_u64().context("sats")?;
            let txid = el.wallet_call("sendtoaddress", json!([addr, btc(sats)]))?;
            println!("faucet: paid {sats} sats of L-BTC to {addr} ({txid})");
            Ok(json!({ "txid": txid }))
        }
        (Method::Post, "/address") => {
            let addr = rt.rpc.get_new_address(None, Some(bitcoincore_rpc::json::AddressType::Bech32m))?.assume_checked();
            Ok(json!({ "address": addr.to_string() }))
        }
        (Method::Post, "/mine") => {
            let n = v["blocks"].as_u64().unwrap_or(1);
            rt.mine(n)?;
            Ok(json!({ "height": rt.height()? }))
        }
        _ => bail!("no {method} {url}"),
    }
}
