//! Lexit's hub: a lichen hub (`lichen_hub`, which runs the
//! contracts) that sells a guaranteed exit from L-BTC. It has no ledger:
//! besides its Bitcoin node it runs an Elements node (elementsd, its
//! wallet holding the hub's L-BTC), and touches Liquid only to mint and to
//! take returns.
//!
//! Its part of a contract ([`LiquidHub`], a [`HubL2`]):
//!
//! - the statement's terms: its deployment (the federation's block signing
//!   script, its return and blinding keys), the checkpoint, the first
//!   Liquid height, the drift; the contract's word `D` they make;
//! - the mint, at the offer: `D − fee` L-BTC paid to the mint key behind
//!   the funding's adaptor point, blinded to her blinding key;
//! - a cooperative withdrawal: her claim's returns checked on Liquid, then
//!   lichen folds;
//! - the dispute's front: when `request` confirms, the hub decides her
//!   claim against Liquid. False: it proves the ground found
//!   (`lexit_statement`) and lichen asserts. True: `request_timeout`
//!   pays her, and the returns are the hub's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use bitcoin::{OutPoint, Sequence, Txid};
use lichen_contract::bit_tree;
use lichen_hub::{Bits, Ctx, FrontStep, HubL2, ProverInput};
use lichen_proto::hp::{Disputed, HpSetup, HpSetupRequest};
use lichen_proto::{fresh_rho, Hash, Layer, OpenRequest, Record};
use lexit_proto::decide::{decide, Found};
use lexit_proto::liquid::{self, LiquidView, Rpc};
use lexit_proto::{statement_outpoint, AliceKeys, Checkpoint, Deployment, HubProves, Keys, LiquidNet, SetupTerms};
use lexit_statement::{Claim, HubInput};
use tiny_http::Method;

pub use lichen_hub::{Config, Hub, OpeningFault};

/// The hub on Liquid: its node, its deployment, its return key's secret,
/// and each dispute's input once its claim is found false.
pub struct LiquidHub {
    pub rpc: Rpc,
    pub deployment: Deployment,
    /// Its fee: a yearly rate on the L-BTC it pays out at an opening (`D`)
    /// and on the BTC it locks beyond it (the liquidity), for the term.
    pub pricing: lichen_proto::Pricing,
    return_secret: SecretKey,
    decided: Mutex<HashMap<u32, (String, HubInput)>>,
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

/// An amount of sats as a decimal of BTC, as the node's RPC takes it.
fn btc(sats: u64) -> String {
    format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)
}

impl LiquidHub {
    /// The hub on `net`, its node at `rpc`, its return key's secret in
    /// `dir` (`liquid.key`), made on first start. The block signing script is the node's tip's.
    pub fn open(dir: &Path, net: LiquidNet, rpc: Rpc, pricing: lichen_proto::Pricing) -> Result<LiquidHub> {
        std::fs::create_dir_all(dir)?;
        let return_secret = load_or_create(&dir.join("liquid.key"))?;
        let secp = Secp256k1::signing_only();
        let hub = return_secret.x_only_public_key(&secp).0.serialize();
        let signblockscript = liquid::signblockscript(&rpc).context("the node's block signing script")?;
        let asset = liquid::node_policy_asset(&rpc)?;
        if let Some(a) = net.policy_asset() {
            anyhow::ensure!(a == asset, "the node's policy asset isn't {net:?}'s L-BTC: another chain?");
        }
        let deployment = Deployment { net, asset, signblockscript, hub };
        Ok(LiquidHub { rpc, deployment, pricing, return_secret, decided: Mutex::new(HashMap::new()) })
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
        let found = decide(&self.rpc, &terms, cid, &deposit, b, &e, &posted, req_time)?;
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
    fn route(&self, _ctx: &dyn Ctx, _method: &Method, _path: &str, _body: &str) -> Option<Result<String>> {
        // payments are Liquid's: the hub serves none
        None
    }

    fn check_owner(&self, _ctx: &dyn Ctx, _owner: &Hash) -> Result<()> {
        Ok(())
    }
    fn setup_terms(&self, ctx: &dyn Ctx, req: &HpSetupRequest) -> Result<serde_json::Value> {
        // fixed now: the checkpoint the Bitcoin tip, the first height
        // Liquid's tip (no return before the contract counts)
        let mut t = SetupTerms { deployment: self.deployment.clone(), checkpoint: Checkpoint::tip(ctx.chain())?, from: self.rpc.height()?, drift: self.deployment.net.drift(), d: [0; 32] };
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
        // never is), blinded to her key
        let k = lexit_proto::keys(record)?;
        let sats = record.minted() - u64::from(fault == OpeningFault::ShortMint);
        let address = liquid::mint_address(&record.l2_address, mint_point, &self.deployment.hub, &k.alice.blinding, self.deployment.net)?;
        let txid = self.rpc.wallet_call("sendtoaddress", serde_json::json!([address.to_string(), btc(sats)]))?;
        let txid = txid.as_str().context("the mint's txid")?.to_string();
        ctx.note(format!("contract {}: minted {sats} sats of L-BTC to the user, locked to the funding: {txid}", record.cid()));
        Ok(serde_json::json!({ "mint_txid": txid }))
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
        match decide(&self.rpc, &terms, cid, &deposit, b, &e, &posted, now)? {
            Found::Holds => {
                ctx.note(format!("contract {cid}: the user's returns of {b} units checked on Liquid"));
                Ok(())
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
