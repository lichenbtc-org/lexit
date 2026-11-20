//! Lexit's part of a lichen contract.
//!
//! A contract is a guaranteed exit from L-BTC: the user pays BTC in and is
//! paid L-BTC on Liquid (the mint, behind the funding's adaptor point:
//! one atomic swap); until `T_close` she can return up to her cap of L-BTC
//! to her contract's return address on Liquid and be paid as much BTC
//! from the contract. There is no ledger of the hub's: L-BTC's is Liquid's.
//!
//! Here: the statement's terms (`D`: the federation's block signing script,
//! L-BTC, the hub's return key, the first Liquid height, the drift, the
//! work `W` on `request`, the unit; [`SetupTerms`]), the user's keys
//! ([`AliceKeys`]: her one-time keys for `b` and `E`, her blinding key for
//! the mint), the front ([`front`], vlichen's request-then-assert),
//! `request` read and proved in its block ([`exit`]), Liquid as both sides
//! see it ([`liquid`]), and a claim decided against it ([`decide`]). The
//! rest of the contract is lichen's (`lichen_proto`).

pub mod decide;
pub mod exit;
pub mod front;
pub mod liquid;
pub mod regtest;

use anyhow::{bail, ensure, Context, Result};
use bitcoin::Amount;
use lichen_proto::{Hash, Net, Record};
use lichen_wots::{WotsParams, WotsPublic};
use serde::{Deserialize, Serialize};

pub use front::{HubProves, Windows};
pub use liquid::LiquidNet;
pub use lexit_statement as statement;

/// Lexit's version (the workspace's): what its hub and wallet
/// report.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The hub's statement guest's image id, hex (`r0/guests/hp.id`; all
/// zeros until the guest is built: the mock statement only).
pub const HP_IMAGE_ID: &str = include_str!("../../../r0/guests/hp.id");

/// The guests a build proves with: a contract's statement pins its image
/// id when it opens, so a hub's must be the same as the wallet's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guests {
    pub hp: String,
}

impl Guests {
    /// This build's.
    pub fn ours() -> Guests {
        Guests { hp: HP_IMAGE_ID.trim().to_string() }
    }
}

/// How many blocks' work, at the checkpoint's difficulty, the statement
/// asks on top of `request`'s block (vlichen's): a hub can't make up a
/// request in a block of its own.
pub const WORK_DEPTH: u128 = 10;

/// The Bitcoin block a contract opened at: its difficulty fixes the work
/// `W` on `request` (vlichen's). Each side checks it against its own view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub height: u32,
    pub hash: bitcoin::BlockHash,
    pub bits: u32,
}

impl Checkpoint {
    /// The checkpoint at the chain's tip.
    pub fn tip(chain: &lichen_chain::Chain) -> Result<Checkpoint> {
        let height = chain.height()?;
        let hash = chain.block_hash(height)?;
        let header = chain.header(&hash)?;
        Ok(Checkpoint { height, hash, bits: u32::from_le_bytes(header[72..76].try_into().unwrap()) })
    }
    /// `W`: [`WORK_DEPTH`] blocks of its difficulty.
    pub fn work(&self) -> u128 {
        WORK_DEPTH.saturating_mul(statement::btc::work(self.bits))
    }
    /// Check it against `chain`: its block on the best chain with these
    /// bits, at most `stale` blocks below the tip.
    pub fn check(&self, chain: &lichen_chain::Chain, stale: u32) -> Result<()> {
        ensure!(chain.block_hash(self.height)? == self.hash, "the checkpoint {} isn't the best chain's block {}", self.hash, self.height);
        let header = chain.header(&self.hash)?;
        ensure!(u32::from_le_bytes(header[72..76].try_into().unwrap()) == self.bits, "the checkpoint's bits aren't its block's");
        ensure!(chain.height()? <= self.height + stale, "the checkpoint, block {}, is stale", self.height);
        Ok(())
    }
}

/// Headers on top of block `from` (internal order), from the chain, until
/// their work reaches `work`: what the statement takes on `request`. An
/// error, saying how far it has got, if not there yet.
pub fn headers_for(chain: &lichen_chain::Chain, from: &Hash, work: u128) -> Result<Vec<[u8; 80]>> {
    use bitcoin::hashes::Hash as _;
    let block = bitcoin::BlockHash::from_byte_array(*from);
    let mut out = vec![];
    let mut sum = 0u128;
    for h in chain.headers_after(&block, 4 * WORK_DEPTH as usize)? {
        sum = sum.saturating_add(statement::btc::work(u32::from_le_bytes(h[72..76].try_into().unwrap())));
        out.push(h);
        if sum >= work {
            return Ok(out);
        }
    }
    bail!("the request isn't deep enough yet: {} blocks on it, {sum} of the {work} work the contract asks", out.len())
}

/// The hub's Liquid deployment, as `GET /info` names it: the network, its
/// policy asset (L-BTC; a regtest chain's own), the federation's block
/// signing script, the hub's return key and its blinding key (each return
/// is blinded to it, so the hub unblinds it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub net: LiquidNet,
    #[serde(with = "lichen_babe::hexbytes::fixed")]
    pub asset: Hash,
    #[serde(with = "hex_vec")]
    pub signblockscript: Vec<u8>,
    #[serde(with = "lichen_babe::hexbytes::fixed")]
    pub hub: Hash,
    pub hub_blinding: bitcoin::secp256k1::PublicKey,
}

/// The statement's terms in a setup (`HpSetup::l2`), the hub's: its
/// deployment, the checkpoint, the first Liquid height a return may be
/// at, the drift, and the contract's word `D` they make with the unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupTerms {
    pub deployment: Deployment,
    pub checkpoint: Checkpoint,
    pub from: u32,
    pub drift: u32,
    #[serde(with = "lichen_babe::hexbytes::fixed")]
    pub d: Hash,
}

impl SetupTerms {
    /// The statement's terms, with the contract's unit.
    pub fn terms(&self, unit: Amount) -> statement::Terms {
        statement::Terms {
            signblockscript: self.deployment.signblockscript.clone(),
            asset: self.deployment.asset,
            generator: liquid::generator(&self.deployment.asset),
            hub: self.deployment.hub,
            from: self.from,
            drift: self.drift,
            work: self.checkpoint.work(),
            unit: unit.to_sat(),
        }
    }
    /// Its `D` is its terms'.
    pub fn check_d(&self, unit: Amount) -> Result<()> {
        ensure!(self.terms(unit).d() == self.d, "the setup's D isn't its terms'");
        Ok(())
    }
}

/// The user's keys (`HpCommit::keys`): her one-time keys for `b` and `E`,
/// and the key her mint is blinded to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliceKeys {
    pub b_key: WotsPublic,
    pub e_key: WotsPublic,
    pub blinding: bitcoin::secp256k1::PublicKey,
}

impl AliceKeys {
    /// Of the parameters the front takes.
    pub fn check(&self) -> Result<()> {
        ensure!(self.b_key.params == WotsParams::for_bytes(4) && self.b_key.digits.len() == 10, "a key for b of other parameters");
        ensure!(self.e_key.params == WotsParams::for_bytes(32), "a key for E of other parameters");
        Ok(())
    }
}

/// A contract's keys (`HpKeys::l2`): hers, the statement's terms, the
/// front's windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Keys {
    pub alice: AliceKeys,
    pub terms: SetupTerms,
    pub windows: Windows,
}

/// The front's window on a network (`w_H`, for the hub's assertion) and
/// her request's latest (`t_a`, before `T_close`). `w_H` covers the
/// request's work `W`, the drift (a cited block still to come) and the
/// proof.
pub fn windows(net: Net, t_close: u32) -> Windows {
    let w_h = match net {
        // blocks every 150 ms in the tests
        Net::Regtest => 80,
        // 30 s blocks: 4 h
        Net::Mutinynet => 480,
        // 10 min blocks: 8 h
        Net::Signet => 48,
        Net::Testnet4 => 72,
    };
    let tail = lichen_proto::hp::windows(net);
    Windows { w_h, t_a: t_close.saturating_sub(u32::from(w_h) + u32::from(tail.delta2) + u32::from(tail.delta1)) }
}

/// A Bitcoin outpoint as the statement takes it.
pub fn statement_outpoint(o: &bitcoin::OutPoint) -> statement::OutPoint {
    use bitcoin::hashes::Hash as _;
    statement::OutPoint { txid: o.txid.to_byte_array(), vout: o.vout }
}

/// The journal of contract `c` with terms' hash `d`, of deposit coin
/// `deposit`: `c ‖ D ‖ H(O_D)`.
pub fn journal_of(c: u32, d: &Hash, deposit: &bitcoin::OutPoint) -> [u8; statement::JOURNAL_LEN] {
    statement::journal(c, d, &statement_outpoint(deposit))
}

/// A record's keys, typed.
pub fn keys(record: &Record) -> Result<Keys> {
    record.dispute.l2()
}

/// The statement's journal for a contract ([`lichen_proto::Layer::journal`]).
pub fn journal(record: &Record) -> Result<Vec<u8>> {
    let k = keys(record)?;
    Ok(journal_of(record.cid(), &k.terms.d, &record.dispute.deposit).to_vec())
}

/// The front of a contract's graph, from its record ([`lichen_proto::Layer::front`]).
pub fn front(record: &Record) -> Result<Box<dyn lichen_babe::graph::Front>> {
    let k = keys(record)?;
    Ok(Box::new(HubProves::new(front::Spec { terms: record.terms, params: lichen_proto::params(record.network), pubs: record.pubs.clone(), b_key: k.alice.b_key.clone(), e_key: k.alice.e_key.clone(), windows: k.windows })?))
}

/// `request`'s claim: `b`, `E` and the claim's bytes.
pub fn requested(request: &bitcoin::Transaction) -> Result<(u32, [u8; 32], Vec<u8>)> {
    let (b, e, posted) = HubProves::read_request(&lichen_proto::setup::link_args(request))?;
    let b = u32::from_be_bytes(b.message().try_into().map_err(|_| anyhow::anyhow!("b's signature signs 4 bytes"))?);
    let e = e.message().try_into().map_err(|_| anyhow::anyhow!("E's signature signs 32 bytes"))?;
    Ok((b, e, posted))
}

/// `request`'s signature on `b`, as the payout's bit outputs take it.
pub fn request_b(request: &bitcoin::Transaction) -> Result<(u32, Vec<Vec<u8>>)> {
    let (b, _, _) = HubProves::read_request(&lichen_proto::setup::link_args(request))?;
    let v = u32::from_be_bytes(b.message().try_into().map_err(|_| anyhow::anyhow!("b's signature signs 4 bytes"))?);
    Ok((v, HubProves::b_wire(&b)))
}

/// A dispute's link `name`, from the chain.
pub fn link(chain: &lichen_chain::Chain, d: &lichen_proto::hp::Disputed, name: &str) -> Result<bitcoin::Transaction> {
    chain.get_tx(&d.txid(name)).with_context(|| format!("{name}, on chain"))
}

mod hex_vec {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        hex::decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
