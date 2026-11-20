//! Lexit's hub: a lichen hub (`lichen_hub`, which runs the contracts)
//! that sells a guaranteed exit from L-BTC. It has no ledger and, in a
//! test deployment, no Liquid node: it reads Liquid from an Esplora (every
//! block it relies on is the federation's, signed; every transaction is
//! tied to its block's root, so the Esplora can delay the hub but not
//! mislead it), and holds its L-BTC in an LWK wallet of its own. A
//! production hub may read and pay through its own elementsd instead
//! ([`LiquidSource::Node`], as the regtest tests do).
//!
//! Its part of a contract ([`LiquidHub`], a [`HubL2`]):
//!
//! - the statement's terms: its deployment (the federation's block signing
//!   script, its return key), the checkpoint, the first Liquid height, the
//!   drift; the contract's word `D` they make;
//! - the mint, at the offer: `D − fee` L-BTC paid to the mint key behind
//!   the funding's adaptor point, blinded to a key of the hub's own for the
//!   contract, its opening in the offer (her wallet checks it);
//! - a cooperative withdrawal: her claim's returns checked on Liquid, then
//!   lichen folds;
//! - the dispute's front: when `request` confirms, the hub decides her
//!   claim against Liquid. False: it proves the ground found
//!   (`lexit_statement`) and lichen asserts. True: `request_timeout`
//!   pays her, and the returns are the hub's;
//! - its chores ([`HubL2::tick`]): refunding abandoned openings' mints,
//!   sweeping claimed returns once their contract has ended, watching its
//!   float.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use bitcoin::{OutPoint, Sequence, Txid};
use lexit_proto::decide::{decide, Found};
use lexit_proto::lbtc::{self, Purse};
use lexit_proto::liquid::{self, Esplora, LiquidView, MintOpening, Rpc};
use lexit_proto::{statement_outpoint, AliceKeys, Checkpoint, Deployment, HubProves, Keys, LiquidNet, SetupTerms};
use lexit_statement::{Citation, Claim, HubInput};
use lichen_contract::bit_tree;
use lichen_hub::{Bits, Ctx, FrontStep, HubL2, ProverInput};
use lichen_proto::hp::{Disputed, HpSetup, HpSetupRequest};
use lichen_proto::{files, fresh_rho, Hash, Layer, OpenRequest, Record};
use sha2::{Digest, Sha256};
use tiny_http::Method;

pub use lichen_hub::{Config, Hub, OpeningFault};

/// Where the hub reads Liquid and keeps its L-BTC's history: an Esplora
/// (a test deployment), or its own elementsd (production; the tests).
pub enum LiquidSource {
    Esplora(String),
    Node(Rpc),
}

/// The hub on Liquid: its view of the chain, its L-BTC, its deployment,
/// its return key's secret, each dispute's input once its claim is found
/// false, and when it last did its chores.
pub struct LiquidHub {
    view: Box<dyn LiquidView + Send + Sync>,
    purse: Mutex<Purse>,
    /// Its mints (`mints/<cid>.json`, for their refunds) and the claims it
    /// took (`claims/<cid>.json`, for their returns' sweep).
    dir: PathBuf,
    pub deployment: Deployment,
    /// Its fee: a yearly rate on the L-BTC it pays out at an opening (`D`)
    /// and on the BTC it locks beyond it (the liquidity), for the term.
    pub pricing: lichen_proto::Pricing,
    return_secret: SecretKey,
    genesis: elements::BlockHash,
    decided: Mutex<HashMap<u32, (String, HubInput)>>,
    /// How often its chores run, how stale its L-BTC wallet may be (a sync
    /// is a full scan: dozens of requests to an Esplora, which a public one
    /// rate-limits), and below what float it warns.
    pub chores_every: Duration,
    pub sync_every: Duration,
    pub float_warn: u64,
    last_chores: Mutex<Option<Instant>>,
    last_sync: Mutex<Option<Instant>>,
}

/// A dispute's front at the hub: `request` confirmed at `at`; `holds` once
/// its claim is found true.
pub struct Front {
    at: u32,
    holds: bool,
}

/// A secret key kept in `path` (hex), made on first use.
fn load_or_create(path: &Path) -> Result<SecretKey> {
    if !path.exists() {
        std::fs::write(path, hex::encode(fresh_rho()))?;
    }
    let b = hex::decode(std::fs::read_to_string(path)?.trim()).with_context(|| format!("{}", path.display()))?;
    Ok(SecretKey::from_slice(&b)?)
}

/// A value kept in `path`, made by `make` on first use.
fn load_or_create_with(path: &Path, make: impl FnOnce() -> Result<String>) -> Result<String> {
    if !path.exists() {
        std::fs::write(path, make()?)?;
    }
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

impl LiquidHub {
    /// The hub on `net`, reading Liquid from `source`; its keys in `dir`,
    /// made on first start (`liquid.key`, the return key's secret;
    /// `mnemonic`, its L-BTC wallet's). The block signing script is the
    /// tip's, as `source` shows it.
    pub fn open(dir: &Path, net: LiquidNet, source: LiquidSource, pricing: lichen_proto::Pricing) -> Result<LiquidHub> {
        std::fs::create_dir_all(dir.join("mints"))?;
        std::fs::create_dir_all(dir.join("claims"))?;
        let return_secret = load_or_create(&dir.join("liquid.key"))?;
        let hub = return_secret.x_only_public_key(&Secp256k1::signing_only()).0.serialize();
        let (view, asset): (Box<dyn LiquidView + Send + Sync>, Hash) = match &source {
            LiquidSource::Esplora(url) => (Box::new(Esplora { url: url.clone() }), net.policy_asset().context("an Esplora hub is on Liquid or its testnet (a regtest chain's asset is its node's)")?),
            LiquidSource::Node(rpc) => {
                let asset = liquid::node_policy_asset(rpc)?;
                if let Some(a) = net.policy_asset() {
                    ensure!(a == asset, "the node's policy asset isn't {net:?}'s L-BTC: another chain?");
                }
                (Box::new(rpc.clone()), asset)
            }
        };
        let signblockscript = liquid::signblockscript(view.as_ref()).context("the tip's block signing script")?;
        let genesis: elements::BlockHash = view.block_hash(0)?.parse()?;
        let network = lbtc::network(net, &asset, Some(genesis))?;
        let mnemonic = load_or_create_with(&dir.join("mnemonic"), || lbtc::new_mnemonic(network))?;
        let backend = match source {
            LiquidSource::Esplora(url) => lbtc::Backend::Esplora(Box::new(lwk_wollet::blocking::EsploraClient::new(&url, network)?)),
            LiquidSource::Node(rpc) => {
                let birth: u32 = load_or_create_with(&dir.join("birth"), || Ok(view.height()?.to_string()))?.parse()?;
                lbtc::Backend::Node(Box::new(lbtc::NodeBackend::new(rpc, birth)))
            }
        };
        let mut purse = Purse::new(&mnemonic, network, backend)?;
        purse.sync()?;
        let deployment = Deployment { net, asset, signblockscript, hub };
        Ok(LiquidHub {
            view,
            purse: Mutex::new(purse),
            dir: dir.to_path_buf(),
            deployment,
            pricing,
            return_secret,
            genesis,
            decided: Mutex::new(HashMap::new()),
            chores_every: Duration::from_secs(300),
            sync_every: Duration::from_secs(60),
            float_warn: 1_000_000,
            last_chores: Mutex::new(None),
            last_sync: Mutex::new(Some(Instant::now())),
        })
    }

    /// The hub's L-BTC wallet, synced if staler than `sync_every` (its own
    /// payments it applies as it makes them, so between syncs it misses
    /// only what it is paid).
    fn purse(&self) -> Result<std::sync::MutexGuard<'_, Purse>> {
        let mut p = self.purse.lock().unwrap();
        let mut last = self.last_sync.lock().unwrap();
        if last.is_none_or(|t| t.elapsed() >= self.sync_every) {
            p.sync()?;
            *last = Some(Instant::now());
        }
        Ok(p)
    }

    /// An address to fund the hub's L-BTC at.
    pub fn address(&self) -> Result<elements::Address> {
        self.purse()?.address()
    }

    /// The hub's L-BTC, in sats.
    pub fn balance(&self) -> Result<u64> {
        self.purse()?.balance()
    }

    /// The key the hub blinds contract `cid`'s mint to: its own, so that it
    /// can always open it.
    fn mint_blinding(&self, cid: u32) -> Result<SecretKey> {
        let k: [u8; 32] = Sha256::new().chain_update(b"lexit/mint-blinding").chain_update(self.return_secret.secret_bytes()).chain_update(cid.to_be_bytes()).finalize().into();
        Ok(SecretKey::from_slice(&k)?)
    }

    fn mint_path(&self, cid: u32) -> PathBuf {
        self.dir.join("mints").join(format!("{cid}.json"))
    }

    fn records<T: serde::de::DeserializeOwned>(&self, kind: &str) -> Result<Vec<(PathBuf, T)>> {
        let mut out = vec![];
        for e in std::fs::read_dir(self.dir.join(kind))? {
            let path = e?.path();
            let r = serde_json::from_slice(&std::fs::read(&path)?).with_context(|| format!("{}", path.display()))?;
            out.push((path, r));
        }
        Ok(out)
    }

    /// A transaction from Liquid.
    fn tx(&self, txid: &str) -> Result<elements::Transaction> {
        Ok(elements::encode::deserialize(&self.view.tx(&internal(txid)?)?)?)
    }

    /// Refund every mint whose contract never opened, once past its
    /// timeout (a mint whose contract opened is the user's, never touched):
    /// how many refunded.
    pub fn refund_abandoned(&self, ctx: &dyn Ctx) -> Result<usize> {
        let opened: Vec<u32> = ctx.contracts().iter().map(|(r, _)| r.cid()).collect();
        let tip = self.view.height()?;
        let timeout = u32::from(self.deployment.net.mint_timeout());
        let mut n = 0;
        for (path, mut m) in self.records::<MintRecord>("mints")? {
            if m.refund.is_some() || m.opened {
                continue;
            }
            if opened.contains(&m.cid) {
                m.opened = true;
                files::write_atomic(&path, &serde_json::to_vec_pretty(&m)?)?;
                continue;
            }
            let Some((height, _)) = self.view.tx_block(&internal(&m.opening.txid)?)? else { continue };
            if tip < height + timeout {
                continue;
            }
            let tx = self.tx(&m.opening.txid)?;
            let purse = self.purse()?;
            let to = purse.address()?;
            let refund = liquid::refund_tx(&tx, m.opening.vout, m.opening.secrets(&self.deployment.asset)?, &m.l2_address, &m.point, &self.return_secret, self.deployment.net, &to, 1_000, self.genesis)?;
            purse.backend.broadcast(&refund)?;
            ctx.note(format!("contract {}: its opening abandoned, the mint refunded: {}", m.cid, refund.txid()));
            m.refund = Some(refund.txid().to_string());
            files::write_atomic(&path, &serde_json::to_vec_pretty(&m)?)?;
            n += 1;
        }
        Ok(n)
    }

    /// Keep a claim the hub took (a fold's, or an exit's that held), for its
    /// returns' sweep once the contract has ended.
    fn keep_claim(&self, cid: u32, claim: &Claim) -> Result<()> {
        let path = self.dir.join("claims").join(format!("{cid}.json"));
        if path.exists() {
            return Ok(());
        }
        let c = ClaimRecord { cid, cited: claim.cited.iter().map(CitedReturn::of).collect(), swept: None };
        Ok(files::write_atomic(&path, &serde_json::to_vec_pretty(&c)?)?)
    }

    /// Sweep the returns of each claim taken whose contract has ended into
    /// the hub's L-BTC: how many claims swept.
    pub fn sweep_returns(&self, ctx: &dyn Ctx) -> Result<usize> {
        let ended: HashMap<u32, Record> = ctx.contracts().into_iter().filter(|(_, closed)| *closed).map(|(r, _)| (r.cid(), r)).collect();
        let secp = elements::secp256k1_zkp::Secp256k1::new();
        let mut n = 0;
        for (path, mut c) in self.records::<ClaimRecord>("claims")? {
            let Some(record) = ended.get(&c.cid).filter(|_| c.swept.is_none()) else { continue };
            let d = lexit_proto::keys(record)?.terms.d;
            let q = liquid::return_secret(&self.return_secret, &d, c.cid)?;
            let keypair = elements::secp256k1_zkp::Keypair::from_seckey_slice(&secp, &q.secret_bytes())?;
            let mut inputs = vec![];
            for r in &c.cited {
                let hash = self.view.block_hash(r.height)?;
                let txid = *self.view.txids(&hash)?.get(r.index as usize).context("a cited transaction")?;
                let tx: elements::Transaction = elements::encode::deserialize(&self.view.tx(&txid)?)?;
                let o = tx.output.get(r.vout as usize).context("a cited output")?.clone();
                let secrets = liquid::secrets_of(&o, &self.deployment.asset, r.value, &hash32(&r.r_a)?, &hash32(&r.r_v)?)?;
                inputs.push((elements::OutPoint::new(tx.txid(), r.vout), o, secrets, keypair));
            }
            let swept = self.purse()?.sweep(inputs)?;
            ctx.note(format!("contract {}: its returns swept into the hub's L-BTC: {}", c.cid, swept.txid()));
            c.swept = Some(swept.txid().to_string());
            files::write_atomic(&path, &serde_json::to_vec_pretty(&c)?)?;
            n += 1;
        }
        Ok(n)
    }

    /// The hub's float: its L-BTC, the mints outstanding (paid, their
    /// contract not yet open, not refunded), and its exposure (the caps of
    /// its open contracts: L-BTC it may have to take back for BTC).
    pub fn float(&self, ctx: &dyn Ctx) -> Result<serde_json::Value> {
        let balance = self.balance()?;
        let address = self.address()?.to_string();
        let outstanding: u64 = self.records::<MintRecord>("mints")?.into_iter().filter(|(_, m)| !m.opened && m.refund.is_none()).map(|(_, m)| m.opening.value).sum();
        let exposure: u64 = ctx.contracts().iter().filter(|(_, closed)| !closed).map(|(r, _)| r.terms.v_max().to_sat()).sum();
        Ok(serde_json::json!({ "balance": balance, "address": address, "outstanding_mints": outstanding, "exposure": exposure }))
    }

    /// The secret of the returns' key, before each contract's tweak.
    pub fn return_secret(&self) -> &SecretKey {
        &self.return_secret
    }

    /// Decide the claim `request` makes: the ground it is false on (the
    /// statement's input built), that it holds, or what is still to come.
    fn decide(&self, ctx: &dyn Ctx, d: &Disputed) -> Result<(Found, Option<HubInput>)> {
        let chain = ctx.chain();
        let record = &d.record;
        let cid = record.cid();
        let k = lexit_proto::keys(record)?;
        let terms = k.terms.terms(record.terms.unit);
        let deposit = statement_outpoint(&record.dispute.deposit);
        let req_id = d.txid("request");
        let req = chain.get_tx(&req_id).context("the request")?;
        let (b, e, posted) = lexit_proto::requested(&req)?;
        let funding = chain.get_tx(&record.funding.txid).context("the funding")?;
        let request = lexit_proto::exit::request_proof(chain, &funding, &req_id)?;
        let req_time = u32::from_le_bytes(request.header[68..72].try_into().unwrap());
        let found = decide(self.view.as_ref(), &terms, cid, &deposit, b, &e, &posted, req_time)?;
        let Found::False(ground, why) = found else { return Ok((found, None)) };
        let req_block = lexit_statement::btc::Header(request.header).hash();
        let headers = match lexit_proto::headers_for(chain, &req_block, terms.work) {
            Ok(h) => h,
            Err(e) => return Ok((Found::Wait(format!("the claim is false ({why}); {e:#}")), None)),
        };
        let input = HubInput { c: cid, deposit, terms, request, headers, ground: ground.clone() };
        Ok((Found::False(ground, why), Some(input)))
    }

    /// Sweep its bit outputs of a payout the user won (`request_timeout`,
    /// `failure`): for each bit clear in her signed `b`, by its leaf
    /// `hub_bit_i` (vlichen's).
    fn sweep_bits(ctx: &dyn Ctx, d: &Disputed) -> Result<Bits> {
        let req = lexit_proto::link(ctx.chain(), d, "request")?;
        let (b, wire) = lexit_proto::request_b(&req)?;
        let k = lexit_proto::keys(&d.record)?;
        let core = d.graph.core();
        let (t, cctx) = (core.terms, core.ctx());
        let mut trees = vec![];
        for i in (t.low_bits..t.bits).filter(|i| (b >> i) & 1 == 0) {
            let tree = bit_tree(&cctx, i, &k.alice.b_key)?;
            trees.push((tree.script_pubkey(), tree, format!("hub_bit_{i}")));
        }
        ctx.sweep(&d.record, trees, &wire, Sequence::ENABLE_RBF_NO_LOCKTIME)
    }
}

impl Layer for LiquidHub {
    fn front(&self, record: &Record) -> Result<Box<dyn lichen_babe::graph::Front>> {
        lexit_proto::front(record)
    }
    fn journal(&self, record: &Record) -> Result<Vec<u8>> {
        lexit_proto::journal(record)
    }
}

fn setup_terms(v: &serde_json::Value) -> Result<SetupTerms> {
    serde_json::from_value(v.clone()).context("the setup's terms")
}

impl HubL2 for LiquidHub {
    type Front = Front;

    fn info(&self) -> serde_json::Value {
        serde_json::json!({ "liquid": self.deployment, "guests": lexit_proto::Guests::ours() })
    }
    fn version(&self) -> String {
        lexit_proto::VERSION.to_string()
    }
    fn pricing(&self) -> Option<lichen_proto::Pricing> {
        Some(self.pricing)
    }
    fn route(&self, ctx: &dyn Ctx, method: &Method, path: &str, _body: &str) -> Option<Result<String>> {
        // payments are Liquid's: the hub serves none. Its float, and its
        // chores on demand (they also run on their own: `tick`)
        match (method, path) {
            (Method::Get, "/liquid/float") => Some(self.float(ctx).map(|v| v.to_string())),
            (Method::Post, "/liquid/refund") => Some(self.refund_abandoned(ctx).map(|n| serde_json::json!({ "refunded": n }).to_string())),
            (Method::Post, "/liquid/sweep") => Some(self.sweep_returns(ctx).map(|n| serde_json::json!({ "swept": n }).to_string())),
            _ => None,
        }
    }
    fn tick(&self, ctx: &dyn Ctx) -> Result<()> {
        {
            let mut last = self.last_chores.lock().unwrap();
            if last.is_some_and(|t| t.elapsed() < self.chores_every) {
                return Ok(());
            }
            *last = Some(Instant::now());
        }
        let refunded = self.refund_abandoned(ctx);
        let swept = self.sweep_returns(ctx);
        let balance = self.balance();
        if let Ok(b) = &balance {
            if *b < self.float_warn {
                ctx.note_once(format!("the hub's L-BTC is low: {b} sats (below {}); fund {}", self.float_warn, self.address().map(|a| a.to_string()).unwrap_or_default()));
            }
        }
        refunded.and(swept).and(balance).map(|_| ())
    }

    fn check_owner(&self, _ctx: &dyn Ctx, _owner: &Hash) -> Result<()> {
        Ok(())
    }
    fn setup_terms(&self, ctx: &dyn Ctx, req: &HpSetupRequest) -> Result<serde_json::Value> {
        // the float first: L-BTC for the mint, at most the deposit coin
        // (D, her reserve and fee budget), before any work on the setup
        let coin = ctx.chain().get_tx(&req.deposit.txid).and_then(|t| t.output.get(req.deposit.vout as usize).map(|o| o.value.to_sat())).context("the deposit coin")?;
        let balance = self.balance()?;
        ensure!(balance >= coin, "the hub's L-BTC ({balance} sats) can't mint this contract now; try another hub, or later");
        // fixed now: the checkpoint the Bitcoin tip, the first height
        // Liquid's tip (no return before the contract counts)
        let mut t = SetupTerms { deployment: self.deployment.clone(), checkpoint: Checkpoint::tip(ctx.chain())?, from: self.view.height()?, drift: self.deployment.net.drift(), d: [0; 32] };
        t.d = t.terms(bitcoin::Amount::from_sat(req.unit)).d();
        Ok(serde_json::to_value(t)?)
    }
    fn same_setup(&self, _setup: &HpSetup, _req: &HpSetupRequest) -> bool {
        true
    }
    fn check_keys(&self, keys: &serde_json::Value) -> Result<()> {
        let k: AliceKeys = serde_json::from_value(keys.clone()).context("the user's keys")?;
        k.check()
    }
    fn setup_journal(&self, cid: u32, setup: &HpSetup, _keys: &serde_json::Value) -> Result<Vec<u8>> {
        let t = setup_terms(&setup.l2)?;
        Ok(lexit_proto::journal_of(cid, &t.d, &setup.deposit).to_vec())
    }
    fn open_keys(&self, ctx: &dyn Ctx, setup: &HpSetup, keys: &serde_json::Value, _req: &OpenRequest, t_close: u32) -> Result<(serde_json::Value, serde_json::Value)> {
        let terms = setup_terms(&setup.l2)?;
        let alice: AliceKeys = serde_json::from_value(keys.clone()).context("the user's keys")?;
        let windows = lexit_proto::windows(ctx.cfg().network, t_close);
        Ok((serde_json::to_value(Keys { alice, terms, windows })?, serde_json::to_value(windows)?))
    }
    fn offered(&self, ctx: &dyn Ctx, record: &Record, mint_point: &PublicKey, fault: OpeningFault) -> Result<serde_json::Value> {
        // D − fee of L-BTC, paid now to a key she can spend once this
        // funding is out (the hub's again after the mint's timeout, if it
        // never is), blinded to a key of the hub's, its opening in the offer
        let cid = record.cid();
        let sats = record.minted() - u64::from(fault == OpeningFault::ShortMint);
        let blinding = self.mint_blinding(cid)?;
        let blinder = PublicKey::from_secret_key(&Secp256k1::signing_only(), &blinding);
        let d = &self.deployment;
        let address = liquid::mint_address(&record.l2_address, mint_point, &d.hub, &blinder, d.net)?;
        let tx = self.purse()?.send(&address, sats)?;
        let vout = tx.output.iter().position(|o| o.script_pubkey == address.script_pubkey()).context("the mint's output")? as u32;
        let (value, _, _, secrets) = lbtc::open(&tx, vout, &blinding.secret_bytes(), &liquid::generator(&d.asset))?;
        let opening = MintOpening { txid: tx.txid().to_string(), vout, value, asset_bf: secrets.asset_bf.to_string(), value_bf: secrets.value_bf.to_string() };
        ensure!(liquid::opens(&tx.output[vout as usize], &opening.secrets(&d.asset)?), "the mint's opening doesn't open it");
        ctx.note(format!("contract {cid}: minted {sats} sats of L-BTC to the user, locked to the funding: {}", opening.txid));
        let m = MintRecord { cid, l2_address: record.l2_address, point: *mint_point, opening: opening.clone(), opened: false, refund: None };
        files::write_atomic(&self.mint_path(cid), &serde_json::to_vec_pretty(&m)?)?;
        Ok(serde_json::json!({ "mint": opening }))
    }
    fn funded(&self, _ctx: &dyn Ctx, _record: &Record) -> Result<()> {
        Ok(())
    }
    fn take_claim(&self, ctx: &dyn Ctx, record: &Record, b: u32, claim: &str, _rest_to: Option<Hash>) -> Result<()> {
        // her returns, each on Liquid as she cites it: the fold pays b
        let posted = hex::decode(claim).context("the claim's hex")?;
        let cid = record.cid();
        let k = lexit_proto::keys(record)?;
        let terms = k.terms.terms(record.terms.unit);
        let deposit = statement_outpoint(&record.dispute.deposit);
        let cl = Claim::from_bytes(&posted).map_err(|r| anyhow::anyhow!("the claim: {r:?}"))?;
        let e = cl.fingerprint(cid, &deposit, b);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as u32;
        match decide(self.view.as_ref(), &terms, cid, &deposit, b, &e, &posted, now)? {
            Found::Holds => {
                ctx.note(format!("contract {cid}: the user's returns of {b} units checked on Liquid"));
                self.keep_claim(cid, &cl)
            }
            Found::False(_, why) => anyhow::bail!("the claim is false: {why}"),
            Found::Wait(why) => anyhow::bail!("not yet: {why}"),
        }
    }

    fn front_start(&self, _ctx: &dyn Ctx, _d: &Disputed, at: u32) -> Result<FrontStep<Front>> {
        Ok(FrontStep::Wait(Front { at, holds: false }))
    }
    fn front_tick(&self, ctx: &dyn Ctx, d: &Disputed, s: Front) -> Result<FrontStep<Front>> {
        let cid = d.record.cid();
        let r_op = OutPoint { txid: d.txid("request"), vout: 0 };
        if !ctx.chain().unspent(&r_op)? && ctx.chain().get_tx(&d.txid("assert")).is_none() {
            ctx.note(format!("contract {cid}: the user's request timed out; she is paid her claim"));
            return Ok(FrontStep::Paid { link: r_op.txid, terminal: "request_timeout".into() });
        }
        let w = lexit_proto::keys(&d.record)?.windows;
        // the assertion must confirm before her timeout
        let last_chance = s.at + u32::from(w.w_h) - (u32::from(w.w_h) / 4).max(2);
        if ctx.chain().height()? >= last_chance {
            ctx.note_once(format!("contract {cid}: too late to assert; request_timeout will pay the user"));
            return Ok(FrontStep::Wait(s));
        }
        if s.holds && !ctx.forging() {
            return Ok(FrontStep::Wait(s));
        }
        match self.decide(ctx, d)? {
            (Found::Wait(why), _) => {
                ctx.note_once(format!("contract {cid}: {why}"));
                Ok(FrontStep::Wait(s))
            }
            // a lying hub (tests) asserts against a claim that holds
            (Found::Holds, _) if ctx.forging() => Ok(FrontStep::Prove { at: s.at }),
            (Found::Holds, _) => {
                ctx.note(format!("contract {cid}: the user's exit claim holds: no assertion; request_timeout will pay her, and her returns are the hub's"));
                let req = lexit_proto::link(ctx.chain(), d, "request")?;
                let (_, _, posted) = lexit_proto::requested(&req)?;
                if let Ok(cl) = Claim::from_bytes(&posted) {
                    self.keep_claim(cid, &cl)?;
                }
                Ok(FrontStep::Wait(Front { at: s.at, holds: true }))
            }
            (Found::False(_, why), Some(input)) => {
                ctx.note(format!("contract {cid}: the user's exit claim is false: {why}"));
                self.decided.lock().unwrap().insert(cid, (why, input));
                Ok(FrontStep::Prove { at: s.at })
            }
            (Found::False(_, why), None) => anyhow::bail!("the claim is false ({why}), with no input"),
        }
    }
    fn prover_input(&self, _ctx: &dyn Ctx, d: &Disputed) -> Result<ProverInput> {
        let cid = d.record.cid();
        let Some((why, input)) = self.decided.lock().unwrap().get(&cid).cloned() else {
            // after a restart: decided again from the front
            return Ok(ProverInput::NotYet("deciding the claim again".into()));
        };
        let journal = lexit_proto::journal(&d.record)?;
        match lexit_statement::check(&input) {
            Ok(j) if j[..] == journal[..] => Ok(ProverInput::Holds(input.to_bytes())),
            Ok(_) => anyhow::bail!("the statement's journal isn't the contract's"),
            Err(r) => Ok(ProverInput::Fails(format!("the ground ({why}) doesn't check natively: {r:?}"))),
        }
    }
    fn collect(&self, ctx: &dyn Ctx, d: &Disputed, _link: Txid, terminal: &str) -> Result<Bits> {
        match terminal {
            // success pays the hub in full
            "success" => Ok(Bits::None),
            _ => Self::sweep_bits(ctx, d),
        }
    }
    fn describe(&self, s: &Front) -> String {
        if s.holds {
            "requested: the claim holds, request_timeout pays the user".into()
        } else {
            "requested: the hub decides the claim against Liquid".into()
        }
    }
}

/// [`HubProves`], re-exported for the tests.
pub type FrontGraph = HubProves;

/// Where the hub keeps its Liquid keys, beside its contracts.
pub fn liquid_dir(dir: &Path) -> PathBuf {
    dir.join("liquid")
}

/// A mint the hub paid: its output and opening, for its refund; whether
/// its contract opened (then it is the user's), its refund if made.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct MintRecord {
    cid: u32,
    #[serde(with = "lichen_babe::hexbytes::fixed")]
    l2_address: Hash,
    point: PublicKey,
    opening: MintOpening,
    opened: bool,
    refund: Option<String>,
}

/// A return a claim cited, as kept for its sweep.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct CitedReturn {
    height: u32,
    index: u32,
    vout: u32,
    value: u64,
    r_a: String,
    r_v: String,
}

impl CitedReturn {
    fn of(c: &Citation) -> CitedReturn {
        CitedReturn { height: c.height, index: c.index, vout: c.vout, value: c.value, r_a: hex::encode(c.r_a), r_v: hex::encode(c.r_v) }
    }
}

/// A claim the hub took: its returns, and their sweep if made.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ClaimRecord {
    cid: u32,
    cited: Vec<CitedReturn>,
    swept: Option<String>,
}

fn hash32(s: &str) -> Result<[u8; 32]> {
    hex::decode(s)?.try_into().map_err(|_| anyhow::anyhow!("32 bytes, hex"))
}

/// A txid as displayed into internal order.
fn internal(s: &str) -> Result<Hash> {
    let mut h = hash32(s)?;
    h.reverse();
    Ok(h)
}
