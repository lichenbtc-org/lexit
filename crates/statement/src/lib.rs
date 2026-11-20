//! The hub's statement (the exit where only the hub proves): that the
//! claim a user posted in `request` is false. Her claim is that she
//! returned L-BTC to the hub for this contract on Liquid: it cites each
//! return by its place in the chain (a block height, a transaction's index,
//! an output) and opens it (its value and blinding factors). The hub proves
//! it false on one of these grounds ([`Ground`]):
//!
//! - **malformed**: the posted bytes aren't a claim, aren't well formed
//!   (one to [`MAX_CITED`] citations, increasing, none below the
//!   contract's first Liquid height), don't hash to the `E` she signed, or
//!   cite less than `b` units in all;
//! - **not a return**: the federation-signed block at a citation's height
//!   has no such transaction or output, or the output doesn't pay the
//!   contract's return key, or its commitments don't open to L-BTC and the
//!   value cited;
//! - **too high**: a federation-signed block below a citation's height
//!   is later than `request`'s block time and the drift: the cited block
//!   didn't exist when she claimed.
//!
//! The claim and `request` are read from Bitcoin as vlichen's statement
//! reads them ([`Request`]: `request` in its block, the contract's work `W`
//! on it). The journal is `c ‖ D ‖ H(O_D)`, fixed at the opening: `D` the
//! contract's terms ([`Terms::d`]), `O_D` its deposit coin.
//!
//! Integers are big-endian in the encodings.

#![no_std]

extern crate alloc;

pub mod btc;
mod codec;
mod request;

use alloc::vec::Vec;
use k256::elliptic_curve::point::AffineCoordinates;
use k256::elliptic_curve::PrimeField;
use k256::{ProjectivePoint, PublicKey, Scalar};
use lexit_elements::tx::{Asset, Value};
use lexit_elements::{commit, header, merkle, tx};
use sha2::{Digest, Sha256};

pub use btc::OutPoint;
pub use request::{posted_items, requested, work_on, Request, Requested, POSTED_ITEM, POSTED_ITEMS};

use codec::{R, W};

pub type Hash = [u8; 32];

/// Why an input is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Encoding,
    /// `request` doesn't spend the contract's funding output, or its
    /// witness isn't a request's.
    Posted,
    /// The headers don't follow `request`'s block, or their work falls
    /// short of `W`.
    Depth,
    /// The ground doesn't hold: the claim is as it says on that count.
    Holds,
}

/// A contract's terms, `D` their hash: the federation's block signing
/// script, L-BTC (its id and generator), the hub's return key, the first
/// Liquid height a return may be at, the drift allowed between Liquid's
/// clock and `request`'s block time, the work `W` on `request`, the unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms {
    pub signblockscript: Vec<u8>,
    pub asset: Hash,
    pub generator: [u8; 33],
    pub hub: Hash,
    pub from: u32,
    pub drift: u32,
    pub work: u128,
    pub unit: u64,
}

impl Terms {
    pub fn d(&self) -> Hash {
        let mut h = Sha256::new();
        h.update(b"lexit/contract");
        h.update((self.signblockscript.len() as u32).to_be_bytes());
        h.update(&self.signblockscript);
        h.update(self.asset);
        h.update(self.generator);
        h.update(self.hub);
        h.update(self.from.to_be_bytes());
        h.update(self.drift.to_be_bytes());
        h.update(self.work.to_be_bytes());
        h.update(self.unit.to_be_bytes());
        h.finalize().into()
    }
    fn write(&self, w: &mut W) {
        w.bytes(&self.signblockscript);
        w.put(&self.asset);
        w.put(&self.generator);
        w.put(&self.hub);
        w.put(&self.from.to_be_bytes());
        w.put(&self.drift.to_be_bytes());
        w.put(&self.work.to_be_bytes());
        w.put(&self.unit.to_be_bytes());
    }
    fn read(r: &mut R) -> Result<Terms, Reject> {
        Ok(Terms {
            signblockscript: r.bytes()?,
            asset: r.take()?,
            generator: r.take()?,
            hub: r.take()?,
            from: r.u32()?,
            drift: r.u32()?,
            work: u128::from_be_bytes(r.take()?),
            unit: u64::from_be_bytes(r.take()?),
        })
    }
}

/// The journal's length in bytes: `c ‖ D ‖ H(O_D)`.
pub const JOURNAL_LEN: usize = 4 + 32 + 32;

/// The journal of contract `c` with terms `d`, of deposit coin `deposit`.
pub fn journal(c: u32, d: &Hash, deposit: &OutPoint) -> [u8; JOURNAL_LEN] {
    let mut j = [0u8; JOURNAL_LEN];
    j[..4].copy_from_slice(&c.to_be_bytes());
    j[4..36].copy_from_slice(d);
    j[36..].copy_from_slice(&Sha256::digest(deposit.to_bytes()));
    j
}

/// The contract's return key: the hub's key `hub` (x-only) tweaked by
/// `SHA256("lexit/return" ‖ hub ‖ D ‖ c)`, x-only. Liquid outputs
/// paying it (`OP_1 <key>`) are this contract's returns, and only its:
/// on chain they are payments to a fresh key, not linked to the hub.
pub fn return_key(hub: &Hash, d: &Hash, c: u32) -> Option<Hash> {
    let mut sec1 = [0x02u8; 33];
    sec1[1..].copy_from_slice(hub);
    let h = PublicKey::from_sec1_bytes(&sec1).ok()?;
    let mut t = Sha256::new();
    t.update(b"lexit/return");
    t.update(hub);
    t.update(d);
    t.update(c.to_be_bytes());
    let t: Hash = t.finalize().into();
    let t = Option::<Scalar>::from(Scalar::from_repr(t.into()))?;
    let q = (ProjectivePoint::from(*h.as_affine()) + ProjectivePoint::GENERATOR * t).to_affine();
    Some(q.x().into())
}

/// The script of the contract's returns: `OP_1 <return key>`.
pub fn return_script(hub: &Hash, d: &Hash, c: u32) -> Option<[u8; 34]> {
    let k = return_key(hub, d, c)?;
    let mut s = [0x51u8; 34];
    s[1] = 0x20;
    s[2..].copy_from_slice(&k);
    Some(s)
}

/// A return cited: where it is (block height, transaction index, output),
/// and its opening (value, in sats; asset and value blinding factors, zero
/// for an explicit output).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Citation {
    pub height: u32,
    pub index: u32,
    pub vout: u32,
    pub value: u64,
    pub r_a: Hash,
    pub r_v: Hash,
}

/// A citation's bytes.
pub const CITATION_LEN: usize = 4 + 4 + 4 + 8 + 32 + 32;

/// At most this many returns in a claim: what `request`'s posted items
/// hold.
pub const MAX_CITED: usize = (POSTED_ITEMS * POSTED_ITEM - 1) / CITATION_LEN;

impl Citation {
    fn place(&self) -> (u32, u32, u32) {
        (self.height, self.index, self.vout)
    }
}

/// A claim as `request` posts it: `[n] citationⁿ`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub cited: Vec<Citation>,
}

impl Claim {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(1 + self.cited.len() * CITATION_LEN);
        b.push(self.cited.len() as u8);
        for c in &self.cited {
            b.extend_from_slice(&c.height.to_be_bytes());
            b.extend_from_slice(&c.index.to_be_bytes());
            b.extend_from_slice(&c.vout.to_be_bytes());
            b.extend_from_slice(&c.value.to_be_bytes());
            b.extend_from_slice(&c.r_a);
            b.extend_from_slice(&c.r_v);
        }
        b
    }
    pub fn from_bytes(b: &[u8]) -> Result<Claim, Reject> {
        let mut r = R(b);
        let n = r.u8()? as usize;
        let mut cited = Vec::with_capacity(n);
        for _ in 0..n {
            cited.push(Citation { height: r.u32()?, index: r.u32()?, vout: r.u32()?, value: u64::from_be_bytes(r.take()?), r_a: r.take()?, r_v: r.take()? });
        }
        r.end()?;
        Ok(Claim { cited })
    }
    /// One to [`MAX_CITED`] citations, in increasing places, none below
    /// `from`.
    pub fn well_formed(&self, from: u32) -> bool {
        (1..=MAX_CITED).contains(&self.cited.len()) && self.cited.windows(2).all(|p| p[0].place() < p[1].place()) && self.cited.iter().all(|c| c.height >= from)
    }
    /// The sats it cites, in all.
    pub fn total(&self) -> u64 {
        self.cited.iter().fold(0u64, |s, c| s.saturating_add(c.value))
    }
    /// `E`, what she signs: `SHA256("lexit/exit" ‖ c ‖ O_D ‖ b ‖
    /// claim)`.
    pub fn fingerprint(&self, c: u32, deposit: &OutPoint, b: u32) -> Hash {
        let mut h = Sha256::new();
        h.update(b"lexit/exit");
        h.update(c.to_be_bytes());
        h.update(deposit.to_bytes());
        h.update(b.to_be_bytes());
        h.update(self.to_bytes());
        h.finalize().into()
    }
}

/// A Liquid block as a ground shows it: its header (with its witness, the
/// federation's signatures), every txid in it, and the cited transaction
/// (empty if the citation's index is out of range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Vec<u8>,
    pub txids: Vec<Hash>,
    pub tx: Vec<u8>,
}

/// Why the claim is false.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    Malformed,
    /// Citation `which` isn't a return: its block shows it.
    NotAReturn { which: u8, block: Block },
    /// Citation `which`'s height wasn't reached in time: a signed header
    /// below it, later than `request`'s block time and the drift.
    TooHigh { which: u8, header: Vec<u8> },
}

/// The statement's input: the contract (`c`, its deposit coin, its terms),
/// `request` in its block and the headers on it, the ground.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubInput {
    pub c: u32,
    pub deposit: OutPoint,
    pub terms: Terms,
    pub request: Request,
    pub headers: Vec<[u8; 80]>,
    pub ground: Ground,
}

impl HubInput {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.put(&self.c.to_be_bytes());
        w.put(&self.deposit.to_bytes());
        self.terms.write(&mut w);
        self.request.write(&mut w);
        w.list(&self.headers);
        match &self.ground {
            Ground::Malformed => w.put(&[0]),
            Ground::NotAReturn { which, block } => {
                w.put(&[1, *which]);
                w.bytes(&block.header);
                w.list(&block.txids);
                w.bytes(&block.tx);
            }
            Ground::TooHigh { which, header } => {
                w.put(&[2, *which]);
                w.bytes(header);
            }
        }
        w.0
    }
    pub fn from_bytes(b: &[u8]) -> Result<HubInput, Reject> {
        let mut r = R(b);
        let c = r.u32()?;
        let deposit = OutPoint::from_bytes(&r.take()?);
        let terms = Terms::read(&mut r)?;
        let request = Request::read(&mut r)?;
        let headers = r.list()?;
        let ground = match r.u8()? {
            0 => Ground::Malformed,
            1 => Ground::NotAReturn { which: r.u8()?, block: Block { header: r.bytes()?, txids: r.list()?, tx: r.bytes()? } },
            2 => Ground::TooHigh { which: r.u8()?, header: r.bytes()? },
            _ => return Err(Reject::Encoding),
        };
        r.end()?;
        Ok(HubInput { c, deposit, terms, request, headers, ground })
    }
}

/// Whether citation `cite` is no return of contract `c` (terms `t`, `D`
/// `d`) in `block`.
pub fn not_a_return(t: &Terms, d: &Hash, c: u32, cite: &Citation, block: &Block) -> bool {
    let Some(h) = header::parse(&block.header) else { return false };
    if !header::signed(&h, &t.signblockscript) || h.height != cite.height || merkle::root(&block.txids) != Some(h.merkle_root) {
        return false;
    }
    let Some(txid) = block.txids.get(cite.index as usize) else {
        // no such transaction in the block
        return block.tx.is_empty();
    };
    let Some(tx) = tx::parse(&block.tx) else { return false };
    if tx.txid != *txid {
        return false;
    }
    let Some(o) = tx.outputs.get(cite.vout as usize) else { return true };
    let Some(script) = return_script(&t.hub, d, c) else { return true };
    if o.script != script {
        return true;
    }
    // the asset: L-BTC, explicit or blinded by r_a; the value v under it
    let generator = match o.asset {
        Asset::Explicit(a) if a == t.asset => t.generator,
        Asset::Confidential(a) if commit::blinded_generator(&t.generator, &cite.r_a) == Some(a) => a,
        _ => return true,
    };
    match o.value {
        Value::Explicit(v) => v != cite.value,
        Value::Confidential(v) => commit::value_commitment(&generator, cite.value, &cite.r_v) != Some(v),
        Value::Null => true,
    }
}

/// The statement: `request`, in its block with the contract's work on it,
/// posts a claim that the ground shows false. Its journal, the contract's.
pub fn check(input: &HubInput) -> Result<[u8; JOURNAL_LEN], Reject> {
    let t = &input.terms;
    let d = t.d();
    let witness = input.request.spending(&input.deposit).ok_or(Reject::Posted)?;
    let req_block = btc::Header(input.request.header);
    if work_on(&req_block.hash(), &input.headers).is_none_or(|w| w < t.work) {
        return Err(Reject::Depth);
    }
    let Requested { b, e, posted } = requested(&witness).ok_or(Reject::Posted)?;
    let claim = Claim::from_bytes(&posted);
    let malformed = match &claim {
        Ok(cl) => !cl.well_formed(t.from) || cl.fingerprint(input.c, &input.deposit, b) != e || cl.total() / t.unit < u64::from(b),
        Err(_) => true,
    };
    let holds = match &input.ground {
        Ground::Malformed => malformed,
        _ if malformed => false,
        Ground::NotAReturn { which, block } => {
            let cl = claim.as_ref().map_err(|_| Reject::Holds)?;
            cl.cited.get(*which as usize).is_some_and(|cite| not_a_return(t, &d, input.c, cite, block))
        }
        Ground::TooHigh { which, header } => {
            let cl = claim.as_ref().map_err(|_| Reject::Holds)?;
            let req_time = u32::from_le_bytes(input.request.header[68..72].try_into().unwrap());
            cl.cited.get(*which as usize).is_some_and(|cite| {
                header::parse(header).is_some_and(|h| header::signed(&h, &t.signblockscript) && h.height < cite.height && u64::from(h.time) > u64::from(req_time) + u64::from(t.drift))
            })
        }
    };
    if holds {
        Ok(journal(input.c, &d, &input.deposit))
    } else {
        Err(Reject::Holds)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;

    fn data(name: &str) -> Vec<u8> {
        let s = std::fs::read_to_string(std::format!("{}/../elements/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        (0..s.trim().len()).step_by(2).map(|i| u8::from_str_radix(&s.trim()[i..i + 2], 16).unwrap()).collect()
    }

    /// Liquid's block signing script.
    fn script() -> Vec<u8> {
        let s = "00207f1a37f651f3cdae15be58ecca344c4c424d3ac250e00452bdb16a9777e6be74";
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn terms() -> Terms {
        Terms { signblockscript: script(), asset: lexit_elements::LBTC, generator: commit::LBTC_GENERATOR, hub: [7; 32], from: 4_000_000, drift: 3 * 3600, work: 0, unit: 1 }
    }

    fn block(index: u32) -> (Block, Citation) {
        let cb = data("tx-coinbase-4153000.hex");
        let t1 = data("tx-1-4153000.hex");
        let txids = vec![tx::parse(&cb).unwrap().txid, tx::parse(&t1).unwrap().txid];
        let tx = match index {
            0 => cb,
            1 => t1,
            _ => vec![],
        };
        (Block { header: data("block-4153000.header.hex"), txids, tx }, Citation { height: 4_153_000, index, vout: 0, value: 1000, r_a: [1; 32], r_v: [2; 32] })
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn a_claim_round_trips_and_fits_the_posted_items() {
        let c = Citation { height: 4_153_000, index: 1, vout: 2, value: 5, r_a: [3; 32], r_v: [4; 32] };
        let claim = Claim { cited: vec![c; 3] };
        assert_eq!(Claim::from_bytes(&claim.to_bytes()), Ok(claim.clone()));
        assert!(!claim.well_formed(0), "repeated places");
        assert_eq!(MAX_CITED, 14);
        assert!(MAX_CITED * CITATION_LEN < POSTED_ITEMS * POSTED_ITEM);
    }

    #[test]
    fn the_return_key_is_the_contracts_own() {
        let hub = [0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98];
        let (a, b) = (return_key(&hub, &[1; 32], 1000).unwrap(), return_key(&hub, &[1; 32], 1001).unwrap());
        assert_ne!(a, b);
        assert_ne!(a, return_key(&hub, &[2; 32], 1000).unwrap());
        assert_eq!(return_script(&hub, &[1; 32], 1000).unwrap()[2..], a);
    }

    #[test]
    fn a_real_block_shows_a_citation_no_return() {
        // the hub's key G (its x)
        let t = Terms { hub: [0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98], ..terms() };
        let d = t.d();
        // a confidential output paying someone else
        let (b, cite) = block(1);
        assert!(not_a_return(&t, &d, 1000, &cite, &b));
        // no such output
        assert!(not_a_return(&t, &d, 1000, &Citation { vout: 9, ..cite }, &b));
        // no such transaction: the block's every txid shows it
        let (b2, cite2) = block(5);
        assert!(not_a_return(&t, &d, 1000, &cite2, &b2));
        // another height than the block's, or another transaction than its
        // txid: no ground
        assert!(!not_a_return(&t, &d, 1000, &Citation { height: 4_153_001, ..cite }, &b));
        assert!(!not_a_return(&t, &d, 1000, &Citation { index: 0, ..cite }, &b));
        // a block not signed under the contract's script: no ground
        let other = Terms { signblockscript: vec![0, 0x20, 1, 2], ..t.clone() };
        assert!(!not_a_return(&other, &d, 1000, &cite, &b));
    }
}
