//! Lexit's wallet: a lichen wallet (`lichen_wallet`, which runs
//! the contracts) whose contracts are guaranteed exits from L-BTC
//! ([`LiquidWallet`], a [`WalletL2`]).
//!
//! Its part of a contract: the hub's terms checked against its own view of
//! Liquid (an Esplora); her keys for `b` and `E` and her blinding key; the
//! mint checked before she signs (on Liquid, unblinded with her key: `D −
//! fee` of L-BTC to the mint key behind the funding's adaptor point), and
//! its key kept once the funding shows `t`; a withdrawal's claim, citing
//! her returns of L-BTC to the contract's return address; the dispute's
//! front ([`front`]).
//!
//! What is still to come (docs/PLAN.md): the Liquid wallet itself (LWK):
//! sweeping the mint, paying a return. Until then a return is paid from
//! any Liquid wallet and recorded here with its blinding factors
//! ([`LiquidWallet::record_return`]).

pub mod front;

use anyhow::{bail, ensure, Context, Result};
use bitcoin::secp256k1::SecretKey;
use bitcoin::{OutPoint, Sequence, Transaction};
use lichen_chain::Chain;
use lichen_contract::{bit_tree, reserve_tree};
use lichen_proto::hp::{Disputed, HpOffer, HpSetup};
use lichen_proto::{Hash, Info, Layer, OpenOffer, Record, Statement};
use lichen_wallet::engine::{ContractSecrets, Engine, Outcome};
use lichen_wallet::{FrontEnd, SweepIn, WalletL2};
use lexit_proto::liquid::{self, Esplora, LiquidView};
use lexit_proto::{AliceKeys, Deployment, Keys, SetupTerms, Windows};
use lexit_statement::{Citation, Claim};
use serde::{Deserialize, Serialize};

pub use lichen_wallet::{dispute, engine, hub_call, hub_info, Ask, Deposit, Held, Opening, Quote, Wallet, PAYOUT_FEERATE};

/// A Lexit wallet.
pub type LexitWallet = Wallet<LiquidWallet>;

/// A return of L-BTC she made for contract `cid`: the output, its value
/// and blinding factors (hex; zero for an explicit output). The factors are
/// the scalars' bytes, big-endian, as secp256k1-zkp takes them: the
/// reverse of what elementsd's RPC displays (`gettransaction`'s
/// `assetblinder`, `amountblinder`), which shows them as it shows hashes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Return {
    pub cid: u32,
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub r_a: String,
    pub r_v: String,
}

/// A mint she holds: the output and its key's secret (hex), hers once the
/// funding showed `t`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mint {
    pub cid: u32,
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub secret: String,
}

/// An exit's plan: the amount claimed, `E`, and what `request` posts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExitPlan {
    pub b: u32,
    pub e: String,
    pub posted: String,
}

impl ExitPlan {
    pub fn e(&self) -> Result<[u8; 32]> {
        hex::decode(&self.e)?.try_into().map_err(|_| anyhow::anyhow!("E of other than 32 bytes"))
    }
    pub fn posted(&self) -> Result<Vec<u8>> {
        Ok(hex::decode(&self.posted)?)
    }
}

/// The L2 in the wallet file: the hub's Liquid deployment (pinned when
/// the wallet was made), the Esplora it reads Liquid by, the mints it
/// holds, the returns it has made.
#[derive(Clone, Serialize, Deserialize)]
pub struct LiquidWallet {
    pub deployment: Deployment,
    pub esplora: String,
    /// Instead of the Esplora, a node (tests, or a user who runs one); not
    /// kept in the file.
    #[serde(skip)]
    pub node: Option<liquid::Rpc>,
    #[serde(default)]
    pub mints: Vec<Mint>,
    #[serde(default)]
    pub returns: Vec<Return>,
}

fn info_l2<T: serde::de::DeserializeOwned>(info: &Info, name: &str) -> Result<T> {
    let v = info.l2.get(name).with_context(|| format!("the hub names no {name}: it runs no Lexit of this wallet's version ({})", lexit_proto::VERSION))?;
    serde_json::from_value(v.clone()).with_context(|| format!("the hub's {name}"))
}

fn plan(v: &serde_json::Value) -> Result<ExitPlan> {
    serde_json::from_value(v.clone()).context("the exit plan")
}

fn hash_hex(s: &str) -> Result<Hash> {
    hex::decode(s)?.try_into().map_err(|_| anyhow::anyhow!("32 bytes, hex"))
}

/// A txid as displayed into internal order.
fn txid_internal(s: &str) -> Result<Hash> {
    let mut h = hash_hex(s)?;
    h.reverse();
    Ok(h)
}

/// The contract secrets of contract `cid`, in opening or open.
fn secrets_of(w: &Wallet<LiquidWallet>, cid: u32) -> Result<ContractSecrets> {
    let nonce = w.contracts.iter().find(|h| h.record.cid() == cid).map(|h| h.nonce.clone()).or_else(|| w.deposits.iter().find(|d| d.cid == cid).map(|d| d.nonce.clone())).with_context(|| format!("no contract {cid}"))?;
    Ok(w.secrets(&nonce))
}

impl LiquidWallet {
    /// Its view of Liquid.
    pub fn view(&self) -> Result<Box<dyn LiquidView>> {
        if let Some(n) = &self.node {
            return Ok(Box::new(n.clone()));
        }
        ensure!(!self.esplora.is_empty(), "no Esplora for Liquid: give one (--esplora-liquid)");
        Ok(Box::new(Esplora { url: self.esplora.clone() }))
    }

    /// Record a return she made for contract `cid` (from any Liquid wallet:
    /// its output and blinding factors).
    pub fn record_return(&mut self, r: Return) -> Result<()> {
        hash_hex(&r.r_a)?;
        hash_hex(&r.r_v)?;
        ensure!(!self.returns.contains(&r), "that return is recorded already");
        self.returns.push(r);
        Ok(())
    }

    /// A return as the claim cites it: its place on Liquid (a block, and
    /// the index there).
    fn cite(&self, r: &Return) -> Result<Citation> {
        let view = self.view()?;
        let txid = txid_internal(&r.txid)?;
        let (height, hash) = view.tx_block(&txid)?.with_context(|| format!("the return {} isn't confirmed on Liquid yet", r.txid))?;
        let index = view.txids(&hash)?.iter().position(|t| *t == txid).context("the return isn't among its block's transactions")? as u32;
        Ok(Citation { height, index, vout: r.vout, value: r.value, r_a: hash_hex(&r.r_a)?, r_v: hash_hex(&r.r_v)? })
    }

    /// The claim of contract `cid` (unit `unit`, deposit coin `deposit`)
    /// for `units`, citing its returns: the exit's plan.
    fn claim(&self, cid: u32, deposit: &bitcoin::OutPoint, unit: u64, units: Option<u32>) -> Result<ExitPlan> {
        let mut cited: Vec<Citation> = self.returns.iter().filter(|r| r.cid == cid).map(|r| self.cite(r)).collect::<Result<_>>()?;
        ensure!(!cited.is_empty(), "no return recorded for contract {cid}: pay L-BTC to its return address first, then record it");
        cited.sort_by_key(|c| (c.height, c.index, c.vout));
        ensure!(cited.len() <= lexit_statement::MAX_CITED, "{} returns: a claim cites {} at most", cited.len(), lexit_statement::MAX_CITED);
        let claim = Claim { cited };
        let most = u32::try_from(claim.total() / unit).unwrap_or(u32::MAX);
        let b = units.unwrap_or(most);
        ensure!(b <= most, "the returns are worth {} sats, less than {b} units of {unit}", claim.total());
        let deposit = lexit_proto::statement_outpoint(deposit);
        Ok(ExitPlan { b, e: hex::encode(claim.fingerprint(cid, &deposit, b)), posted: hex::encode(claim.to_bytes()) })
    }
}

impl Layer for LiquidWallet {
    fn front(&self, record: &Record) -> Result<Box<dyn lichen_babe::graph::Front>> {
        lexit_proto::front(record)
    }
    fn journal(&self, record: &Record) -> Result<Vec<u8>> {
        lexit_proto::journal(record)
    }
}

impl WalletL2 for LiquidWallet {
    fn create(info: &Info) -> Result<LiquidWallet> {
        let deployment: Deployment = info_l2(info, "liquid")?;
        let esplora = deployment.net.default_esplora().unwrap_or_default().to_string();
        let w = LiquidWallet { deployment, esplora, node: None, mints: vec![], returns: vec![] };
        w.check_info(info)?;
        Ok(w)
    }
    fn check_info(&self, info: &Info) -> Result<()> {
        let d: Deployment = info_l2(info, "liquid")?;
        ensure!(d == self.deployment, "the hub's Liquid deployment isn't this wallet's (its return key {}, ours {})", hex::encode(d.hub), hex::encode(self.deployment.hub));
        if let Some(a) = d.net.policy_asset() {
            ensure!(a == d.asset, "the hub names another asset than {:?}'s L-BTC", d.net);
        }
        let guests: lexit_proto::Guests = info_l2(info, "guests")?;
        ensure!(guests == lexit_proto::Guests::ours(), "the hub proves with other guests than this wallet's: it runs Lexit {}, this wallet {}", if info.version.is_empty() { "?" } else { &info.version }, lexit_proto::VERSION);
        Ok(())
    }
    fn version() -> String {
        lexit_proto::VERSION.to_string()
    }

    fn setup_terms(_w: &Wallet<Self>, _cid: u32, _quote: &Quote, _owner: &Hash) -> Result<serde_json::Value> {
        // the hub fixes them; she checks them
        Ok(serde_json::Value::Null)
    }
    fn check_setup(w: &Wallet<Self>, chain: &Chain, setup: &HpSetup, _asked: &serde_json::Value) -> Result<()> {
        let t: SetupTerms = serde_json::from_value(setup.l2.clone()).context("the setup's terms")?;
        ensure!(t.deployment == w.l2.deployment, "the setup's deployment isn't the hub's");
        ensure!(t.drift == t.deployment.net.drift(), "the setup's drift, {} s, isn't the network's", t.drift);
        t.checkpoint.check(chain, 2016)?;
        t.check_d(bitcoin::Amount::from_sat(setup.unit))?;
        // Liquid as she sees it: the federation's script, and the first
        // height not ahead of the tip (nor long behind it)
        let view = w.l2.view()?;
        ensure!(liquid::signblockscript(view.as_ref())? == t.deployment.signblockscript, "the hub's block signing script isn't Liquid's tip's");
        let tip = view.height()?;
        ensure!(t.from <= tip && t.from + 120 >= tip, "the setup's first Liquid height {} isn't Liquid's tip ({tip})", t.from);
        Ok(())
    }
    fn alice_keys(s: &ContractSecrets) -> serde_json::Value {
        serde_json::to_value(front::alice_keys(s)).expect("keys serialize")
    }
    fn setup_journal(cid: u32, setup: &HpSetup, _keys: &serde_json::Value) -> Result<Vec<u8>> {
        let t: SetupTerms = serde_json::from_value(setup.l2.clone()).context("the setup's terms")?;
        Ok(lexit_proto::journal_of(cid, &t.d, &setup.deposit).to_vec())
    }
    fn record_keys(_w: &Wallet<Self>, setup: &HpSetup, keys: &serde_json::Value, offer: &HpOffer, _quote: &Quote) -> Result<serde_json::Value> {
        let windows: Windows = serde_json::from_value(offer.l2.clone()).context("the front's windows in the hub's offer")?;
        let alice: AliceKeys = serde_json::from_value(keys.clone())?;
        Ok(serde_json::to_value(Keys { alice, terms: serde_json::from_value(setup.l2.clone())?, windows })?)
    }
    fn check_offer(w: &Wallet<Self>, record: &Record, offer: &OpenOffer) -> Result<()> {
        let k = lexit_proto::keys(record)?;
        let want = lexit_proto::windows(record.network, offer.t_close);
        ensure!(k.windows == want, "the front's windows {:?}, not the network's {want:?}", k.windows);
        // D − fee of the policy asset, to the mint key behind this
        // funding's adaptor point, confirmed on Liquid before she signs
        let txid = offer.l2_offer.get("mint_txid").and_then(|v| v.as_str()).context("the offer names no mint")?;
        let view = w.l2.view()?;
        let blinding = front::blinding(&secrets_of(w, record.cid())?);
        let blinder = bitcoin::secp256k1::PublicKey::from_secret_key(&bitcoin::secp256k1::Secp256k1::signing_only(), &blinding);
        let d = &w.l2.deployment;
        let spk = liquid::mint_address(&record.l2_address, &offer.mint_point, &d.hub, &blinder, d.net)?.script_pubkey();
        let mut waited = 0;
        loop {
            if view.tx_block(&txid_internal(txid)?)?.is_some() {
                break;
            }
            ensure!(waited < 600, "the mint {txid} isn't confirmed on Liquid after {waited} s");
            std::thread::sleep(std::time::Duration::from_secs(5));
            waited += 5;
        }
        let tx: elements::Transaction = elements::encode::deserialize(&view.tx(&txid_internal(txid)?)?)?;
        let secp = elements::secp256k1_zkp::Secp256k1::new();
        let sk = elements::secp256k1_zkp::SecretKey::from_slice(&blinding.secret_bytes())?;
        let asset = w.l2.deployment.asset;
        let paid = tx.output.iter().filter(|o| o.script_pubkey == spk).filter_map(|o| o.unblind(&secp, sk).ok()).any(|s| elements::encode::serialize(&s.asset)[..] == asset[..] && s.value == record.minted());
        ensure!(paid, "the mint {txid} doesn't pay {} sats of L-BTC to the mint key", record.minted());
        Ok(())
    }
    fn minted(w: &mut Wallet<Self>, held: &Held, t: &SecretKey) -> Result<()> {
        // her mint key's secret (liquid::mint_secret); sweeping it waits
        // for the Liquid wallet
        let locked = held.mint.as_ref().context("no mint offered")?;
        let txid = locked.l2.get("mint_txid").and_then(|v| v.as_str()).context("the offer names no mint")?.to_string();
        let a = w.secrets(&held.nonce).l2();
        ensure!(a.x_only_public_key().0.serialize() == held.record.l2_address, "the contract's L2 address isn't its key's");
        let secret = liquid::mint_secret(&a, t)?;
        let cid = held.record.cid();
        w.l2.mints.retain(|m| m.cid != cid);
        w.l2.mints.push(Mint { cid, txid, vout: 0, value: held.record.minted(), secret: hex::encode(secret.secret_bytes()) });
        w.save()
    }

    fn withdrawal(w: &mut Wallet<Self>, _chain: &Chain, cid: u32, units: u32) -> Result<serde_json::Value> {
        let h = w.contracts.iter().find(|h| h.record.cid() == cid).with_context(|| format!("no contract {cid}"))?;
        Ok(serde_json::to_value(w.l2.claim(cid, &h.record.dispute.deposit, h.record.terms.unit.to_sat(), Some(units))?)?)
    }
    fn plan_b(p: &serde_json::Value) -> Result<u32> {
        Ok(plan(p)?.b)
    }
    fn plan_claim(p: &serde_json::Value) -> Result<String> {
        Ok(plan(p)?.posted)
    }
    fn rest_to(_w: &Wallet<Self>) -> Option<Hash> {
        None
    }
    fn folded(w: &mut Wallet<Self>, cid: u32, _units: u32) -> Result<()> {
        w.l2.returns.retain(|r| r.cid != cid);
        Ok(())
    }
    fn exit_plan(w: &mut Wallet<Self>, _chain: &Chain, cid: u32) -> Result<serde_json::Value> {
        let h = w.contracts.iter().find(|h| h.record.cid() == cid).with_context(|| format!("no contract {cid}"))?;
        let p = w.l2.claim(cid, &h.record.dispute.deposit, h.record.terms.unit.to_sat(), None)?;
        // b at most the cap, in units
        let cap = u32::try_from(h.record.terms.v_max().to_sat() / h.record.terms.unit.to_sat()).unwrap_or(u32::MAX);
        if p.b > cap {
            bail!("the returns are worth {} units, beyond the contract's cap of {cap}", p.b);
        }
        Ok(serde_json::to_value(p)?)
    }
    fn exited(w: &mut Wallet<Self>, cid: u32) -> Result<()> {
        w.l2.returns.retain(|r| r.cid != cid);
        Ok(())
    }
    fn unlocked(_w: &mut Wallet<Self>, _k: usize, _z: &[u8]) -> Result<Option<u64>> {
        Ok(None)
    }
    fn run_front(e: &mut Engine<'_>, chain: &Chain, d: &Disputed, p: &serde_json::Value) -> Result<FrontEnd> {
        front::run(e, chain, d, &plan(p)?)
    }
    fn payout_inputs(w: &Wallet<Self>, _chain: &Chain, held: &Held, _statement: &Statement, _outcome: &Outcome, payout: &Transaction, b: u32) -> Result<(Vec<SweepIn>, Vec<Vec<u8>>, u16)> {
        let txid = payout.compute_txid();
        let k = lexit_proto::keys(&held.record)?;
        let g = lexit_proto::front(&held.record)?;
        let core = g.core();
        let (t, ctx) = (core.terms, core.ctx());
        let delta = core.params.delta;
        let at_delta = Sequence::from_height(delta);
        let mut ins: Vec<SweepIn> = vec![];
        // her outputs: the bits set in b, then the reserves
        for i in t.low_bits..t.bits {
            if (b >> i) & 1 == 1 {
                let vout = i - t.low_bits;
                ins.push((OutPoint { txid, vout }, payout.output[vout as usize].clone(), at_delta, Some((bit_tree(&ctx, i, &k.alice.b_key)?, format!("alice_bit_{i}"), true))));
            }
        }
        let vout = t.bits - t.low_bits;
        ins.push((OutPoint { txid, vout }, payout.output[vout as usize].clone(), at_delta, Some((reserve_tree(&ctx, t, &k.alice.b_key)?, "alice_reserve".to_string(), false))));
        Ok((ins, front::b_wire(&w.secrets(&held.nonce), b)?, delta))
    }
}

/// The return address of contract `cid` (one she holds).
pub fn return_address(w: &LexitWallet, cid: u32) -> Result<elements::Address> {
    let h = w.contracts.iter().find(|h| h.record.cid() == cid).with_context(|| format!("no contract {cid}"))?;
    let k = lexit_proto::keys(&h.record)?;
    let d = &w.l2.deployment;
    liquid::return_address(&d.hub, &k.terms.d, cid, &d.hub_blinding, d.net)
}
