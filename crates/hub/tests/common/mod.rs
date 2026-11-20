//! What the hub's tests share: a regtest bitcoind and an Elements regtest
//! chain of our own (`lexit_proto::regtest`, our 2-of-3 signing its
//! blocks), a Liquid block every 300 ms, hubs on the mock statement,
//! wallets reading Liquid through the node, returns paid from a node
//! wallet with their blinding factors.
//!
//! Needs `bitcoind` (`LICHEN_BITCOIND`) and `elementsd` (`LEXIT_ELEMENTSD`).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitcoin::Amount;
use bitcoincore_rpc::{Auth, RpcApi};
use lexit_elements::commit;
use lexit_elements::tx::{self, Asset, Value};
use lexit_hub::{Config, LiquidHub};
use lexit_proto::liquid::{LiquidNet, LiquidView, Rpc};
use lexit_proto::regtest::{mine_with, ElementsRegtest, Federation};
use lexit_wallet::{LexitWallet as Wallet, LiquidWallet, Return};
use lichen_btc::regtest::Regtest;
use lichen_chain::Chain;
use lichen_proto::Statement;
use lichen_wallet::WalletL2;

pub type Hub = lexit_hub::Hub<LiquidHub>;

pub fn statement() -> Statement {
    Statement::parse("hub-proves-mock", lichen_babe::statement::image_id(lexit_proto::HP_IMAGE_ID)).unwrap()
}

pub fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(120), "{what}: timed out");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// An amount of sats as a decimal of BTC, as the node's RPC takes it.
fn btc(sats: u64) -> String {
    format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)
}

pub struct Env {
    pub rt: Regtest,
    pub el: ElementsRegtest,
    pub dir: PathBuf,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Env {
    /// Both chains, a Liquid block every 300 ms; `name` names the
    /// directory.
    pub fn new(name: &str) -> Env {
        let rt = Regtest::start_standard().unwrap();
        let el = ElementsRegtest::start(Federation::regtest()).unwrap();
        let dir = std::env::temp_dir().join(format!("lexit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut env = Env { rt, el, dir, stop: Arc::new(AtomicBool::new(false)), threads: vec![] };
        let (rpc, fed) = (env.el.rpc.clone(), env.el.federation.clone());
        env.background(Duration::from_millis(300), move || {
            let _ = mine_with(&rpc, &fed, 1);
        });
        env
    }

    fn background(&mut self, every: Duration, f: impl Fn() + Send + 'static) {
        let s = self.stop.clone();
        self.threads.push(std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                f();
                std::thread::sleep(every);
            }
        }));
    }

    /// `f`, with a Bitcoin block every 150 ms meanwhile.
    pub fn mining<T>(&self, f: impl FnOnce() -> T) -> T {
        let rpc = bitcoincore_rpc::Client::new(&format!("{}/wallet/harness", self.rt.rpc_url()), Auth::CookieFile(self.rt.cookie().clone())).unwrap();
        let addr = rpc.get_new_address(None, None).unwrap().assume_checked();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let t = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let _ = rpc.generate_to_address(1, &addr);
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let r = f();
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        r
    }

    pub fn chain(&self, wallet: &str) -> Chain {
        Chain::connect(self.rt.rpc_url(), self.rt.cookie(), wallet).unwrap()
    }

    /// An Elements wallet `name`, given `sats` of L-BTC from the free
    /// coins, confirmed.
    pub fn liquid_wallet(&self, name: &str, sats: u64) -> Rpc {
        let w = self.el.wallet(name).unwrap();
        let addr = w.wallet_call("getnewaddress", serde_json::json!([])).unwrap();
        let txid = self.el.rpc.wallet_call("sendtoaddress", serde_json::json!([addr, btc(sats)])).unwrap();
        let txid = internal(txid.as_str().unwrap());
        wait("the L-BTC confirmed", || w.tx_block(&txid).unwrap().is_some());
        w
    }

    /// A hub on bitcoind wallet `name` and Elements wallet `name` (each
    /// funded), its config the tests' but for `f`'s changes; no fee.
    pub fn hub(&self, name: &str, f: impl FnOnce(&mut Config)) -> Hub {
        self.rt.fund(&self.chain(name).new_spk().unwrap(), Amount::from_sat(100_000_000)).unwrap();
        self.rt.mine(1).unwrap();
        let liquid = self.liquid_wallet(name, 50_000_000);
        let mut cfg = Config {
            network: lichen_proto::Net::Regtest,
            listen: "127.0.0.1:0".into(),
            rpc_url: self.rt.rpc_url().to_string(),
            cookie: self.rt.cookie().clone(),
            wallet: name.into(),
            statement: statement(),
            dir: self.dir.join(name),
            seed: format!("test-{name}"),
            prover: None,
            reserve_ppm: 20_000,
            fee_ppm: 0,
            max_liquidity: 10_000_000,
            duration: 2_000,
            refuse_folds: false,
            forge_assertions: false,
            first_cid: 1000,
        };
        f(&mut cfg);
        let l2 = LiquidHub::open(&cfg.dir.join("liquid"), LiquidNet::Regtest, liquid, lichen_proto::Pricing::default()).unwrap();
        Hub::start(cfg, l2).unwrap()
    }

    /// A user: a funded bitcoind wallet, and a Lexit wallet for `hub`
    /// reading Liquid through the node.
    pub fn user(&self, name: &str, hub: &Hub) -> (Wallet, Chain) {
        let ch = self.chain(name);
        self.rt.fund(&ch.new_spk().unwrap(), Amount::from_sat(10_000_000)).unwrap();
        self.rt.mine(1).unwrap();
        let mut l2 = LiquidWallet::create(&lexit_wallet::hub_info(&hub.url).unwrap()).unwrap();
        l2.node = Some(self.el.rpc.clone());
        let mut w = Wallet::create_with(&self.dir.join(format!("{name}.json")), &hub.url, l2).unwrap();
        let label = name.to_string();
        w.progress = Some(Arc::new(move |m: String| println!("LEXIT {label}: {m}")));
        (w, ch)
    }

    /// `sats` of L-BTC paid from Elements wallet `from` to `address`,
    /// confirmed: the return as the wallet records it, its blinding
    /// factors from the node, in the byte order that opens the output's
    /// commitments (asserted).
    pub fn pay(&self, from: &Rpc, cid: u32, address: &str, sats: u64) -> Return {
        let txid = from.wallet_call("sendtoaddress", serde_json::json!([address, btc(sats)])).unwrap();
        let txid = txid.as_str().unwrap().to_string();
        let id = internal(&txid);
        wait("the payment confirmed", || from.tx_block(&id).unwrap().is_some());
        let raw = from.tx(&id).unwrap();
        let t = tx::parse(&raw).unwrap();
        let spk = from.call("getaddressinfo", serde_json::json!([address])).ok().and_then(|i| i["scriptPubKey"].as_str().map(|s| hex::decode(s).unwrap()));
        let details = from.wallet_call("gettransaction", serde_json::json!([txid])).unwrap();
        for d in details["details"].as_array().unwrap() {
            let vout = d["vout"].as_u64().unwrap() as u32;
            let o = &t.outputs[vout as usize];
            if spk.as_deref().is_some_and(|s| s != o.script) {
                continue;
            }
            let (Some(ab), Some(vb)) = (d["assetblinder"].as_str(), d["amountblinder"].as_str()) else { continue };
            for reverse in [false, true] {
                let f = |h: &str| -> [u8; 32] {
                    let mut b: [u8; 32] = hex::decode(h).unwrap().try_into().unwrap();
                    if reverse {
                        b.reverse();
                    }
                    b
                };
                let (r_a, r_v) = (f(ab), f(vb));
                let opens = match (o.asset, o.value) {
                    (Asset::Confidential(a), Value::Confidential(c)) => commit::opens(&a, &c, &lexit_proto::liquid::generator(&policy_asset(from)), sats, &r_a, &r_v),
                    _ => false,
                };
                if opens {
                    println!("LEXIT: the node's blinders open the output {}reversed", if reverse { "" } else { "not " });
                    return Return { cid, txid, vout, value: sats, r_a: hex::encode(r_a), r_v: hex::encode(r_v) };
                }
            }
        }
        panic!("no output of {txid} paying {address} opens with the node's blinders: {details}");
    }
}

/// The chain's policy asset (internal order).
pub fn policy_asset(rpc: &Rpc) -> [u8; 32] {
    lexit_proto::liquid::node_policy_asset(rpc).unwrap()
}

/// A txid as displayed into internal order.
pub fn internal(s: &str) -> [u8; 32] {
    let mut h: [u8; 32] = hex::decode(s).unwrap().try_into().unwrap();
    h.reverse();
    h
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
