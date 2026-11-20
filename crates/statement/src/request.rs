//! `request` as the statement reads it from Bitcoin (vlichen's,
//! `vlichen_withdraw`): in its block, with its witness, the contract's
//! funding transaction it spends, and the headers on its block carrying
//! the contract's work `W`. Her signatures on `b` and `E` aren't checked
//! here: Bitcoin checked them, in `request`'s script; only their digits
//! are read.

use alloc::vec::Vec;

use crate::btc::{self, work, Header, OutPoint};
use crate::codec::{R, W};
use crate::{Hash, Reject};

/// `request`'s witness carries the claim in its first this many items,
/// concatenated: as many as the script drops, empty ones padding.
pub const POSTED_ITEMS: usize = 15;

/// The largest item: Bitcoin Core relays no tapscript witness item larger.
pub const POSTED_ITEM: usize = 80;

/// A claim's bytes as `request`'s witness carries them: [`POSTED_ITEMS`]
/// items of up to [`POSTED_ITEM`] bytes, bottom first. None if they don't
/// fit.
pub fn posted_items(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    (bytes.len() <= POSTED_ITEMS * POSTED_ITEM).then(|| (0..POSTED_ITEMS).map(|i| bytes.get(i * POSTED_ITEM..).map_or(&[][..], |b| &b[..b.len().min(POSTED_ITEM)])).collect())
}

/// `request` in a block, with its witness: the transaction (witness
/// serialization), the block's header, the transaction's index and path
/// in the block's witness tree, and the block's coinbase (non-witness),
/// its path in the txid tree (which fixes the trees' depth) and its
/// witness reserved value; and the contract's funding transaction
/// (non-witness), whose output 0 `request` spends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub funding: Vec<u8>,
    pub tx: Vec<u8>,
    pub header: [u8; 80],
    pub index: u32,
    pub path: Vec<Hash>,
    pub coinbase: Vec<u8>,
    pub coinbase_path: Vec<Hash>,
    pub reserved: Hash,
}

impl Request {
    /// The witness of the input spending the contract's funding output, the
    /// funding the transaction spending `deposit` (the contract's deposit
    /// coin: nothing else can spend it): none unless the transaction spends
    /// it and is in the header's block.
    pub fn spending(&self, deposit: &OutPoint) -> Option<Vec<&[u8]>> {
        if !btc::parse_tx(&self.funding)?.inputs.contains(deposit) {
            return None;
        }
        let funding = OutPoint { txid: btc::sha256d(&self.funding), vout: 0 };
        if !btc::witness_in_block(&self.tx, &Header(self.header), self.index, &self.path, &self.coinbase, &self.coinbase_path, &self.reserved) {
            return None;
        }
        let t = btc::parse_witness_tx(&self.tx)?;
        let i = t.tx.inputs.iter().position(|o| *o == funding)?;
        t.witness.into_iter().nth(i)
    }

    pub(crate) fn write(&self, w: &mut W) {
        w.bytes(&self.funding);
        w.bytes(&self.tx);
        w.put(&self.header);
        w.put(&self.index.to_be_bytes());
        w.list(&self.path);
        w.bytes(&self.coinbase);
        w.list(&self.coinbase_path);
        w.put(&self.reserved);
    }

    pub(crate) fn read(r: &mut R) -> Result<Request, Reject> {
        Ok(Request { funding: r.bytes()?, tx: r.bytes()?, header: r.take()?, index: r.u32()?, path: r.list()?, coinbase: r.bytes()?, coinbase_path: r.list()?, reserved: r.take()? })
    }
}

/// The work of headers on top of block `from`, if each follows the last
/// and is within its own target.
pub fn work_on(from: &Hash, headers: &[[u8; 80]]) -> Option<u128> {
    let mut prev = *from;
    let mut sum = 0u128;
    for h in headers {
        let h = Header(*h);
        if h.prev() != prev || !h.pow_ok() {
            return None;
        }
        sum = sum.saturating_add(work(h.bits()));
        prev = h.hash();
    }
    Some(sum)
}

/// What `request` carries: `b`, `E` and the claim's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requested {
    pub b: u32,
    pub e: Hash,
    pub posted: Vec<u8>,
}

/// A Winternitz signature's message from its witness items, bottom first
/// (`hash_0, digit_0, hash_1, digit_1, …`): its first `bytes` bytes'
/// digits.
fn signed_message(items: &[&[u8]], bytes: usize) -> Option<Vec<u8>> {
    let digit = |i: usize| -> Option<u8> {
        match items.get(2 * i + 1)? {
            [] => Some(0),
            [d] if *d <= 15 => Some(*d),
            _ => None,
        }
    };
    (0..bytes).map(|k| Some((digit(2 * k)? << 4) | digit(2 * k + 1)?)).collect()
}

/// The checksum digits of a signature on `bytes` bytes (lichen-wots's
/// `WotsParams::for_bytes`).
fn checksum_digits(bytes: usize) -> usize {
    let most = 15 * 2 * bytes as u64;
    (1..).find(|n| 16u64.pow(*n) > most).unwrap() as usize
}

/// `request`'s witness (the input spending the contract's funding output,
/// bottom first): the claim in its first [`POSTED_ITEMS`] items, then
/// Alice's signature on `E` (32 bytes), then on `b` (4), each a hash and a
/// digit a digit, below the 2-of-2's two signatures, the script and the
/// control block.
pub fn requested(witness: &[&[u8]]) -> Option<Requested> {
    let e_items = 2 * (64 + checksum_digits(32));
    let b_items = 2 * (8 + checksum_digits(4));
    if witness.len() != POSTED_ITEMS + e_items + b_items + 4 {
        return None;
    }
    let posted = witness[..POSTED_ITEMS].concat();
    let e = signed_message(&witness[POSTED_ITEMS..POSTED_ITEMS + e_items], 32)?;
    let b = signed_message(&witness[POSTED_ITEMS + e_items..POSTED_ITEMS + e_items + b_items], 4)?;
    Some(Requested { b: u32::from_be_bytes(b.try_into().ok()?), e: e.try_into().ok()?, posted })
}
