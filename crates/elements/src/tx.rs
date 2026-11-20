//! An Elements transaction:
//!
//! ```text
//! version (4) ‖ flag (1: 0, or 1 with witnesses) ‖ inputs ‖ outputs ‖ locktime (4)
//! ‖ [witnesses: per input, then per output]
//! ```
//!
//! An input is a prevout (its index's top bits flag an issuance and a
//! peg-in), a scriptSig, a sequence, and an issuance if flagged. An output
//! is an asset, a value and a nonce, each explicit or confidential, and a
//! script. The txid hashes the transaction with flag 0 and no witnesses.

use alloc::vec::Vec;

use crate::read::Rd;
use crate::{sha256d, Hash};

/// An output's asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asset {
    Null,
    /// The asset id (internal order).
    Explicit(Hash),
    /// A blinded generator, `0a`/`0b` and an x coordinate.
    Confidential([u8; 33]),
}

/// An output's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Explicit(u64),
    /// A Pedersen commitment, `08`/`09` and an x coordinate.
    Confidential([u8; 33]),
}

/// An output: what it holds and the script that locks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output<'a> {
    pub asset: Asset,
    pub value: Value,
    /// Its nonce: empty, or 33 bytes (the ECDH key a blinded output's
    /// owner unblinds it with).
    pub nonce: &'a [u8],
    pub script: &'a [u8],
}

/// An input's spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub txid: Hash,
    /// The index, without its flags.
    pub vout: u32,
    pub issuance: bool,
    pub pegin: bool,
}

/// A transaction read from its serialization (with or without witnesses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tx<'a> {
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output<'a>>,
    /// Its txid (internal order).
    pub txid: Hash,
}

const ISSUANCE: u32 = 1 << 31;
const PEGIN: u32 = 1 << 30;
const INDEX: u32 = (1 << 30) - 1;

fn asset(r: &mut Rd) -> Option<Asset> {
    Some(match r.u8()? {
        0 => Asset::Null,
        1 => Asset::Explicit(r.array()?),
        p @ (0x0a | 0x0b) => {
            let mut c = [p; 33];
            c[1..].copy_from_slice(r.take(32)?);
            Asset::Confidential(c)
        }
        _ => return None,
    })
}

fn value(r: &mut Rd) -> Option<Value> {
    Some(match r.u8()? {
        0 => Value::Null,
        1 => Value::Explicit(u64::from_be_bytes(r.array()?)),
        p @ (0x08 | 0x09) => {
            let mut c = [p; 33];
            c[1..].copy_from_slice(r.take(32)?);
            Value::Confidential(c)
        }
        _ => return None,
    })
}

fn nonce<'a>(r: &mut Rd<'a>) -> Option<&'a [u8]> {
    let start = r.p;
    match r.u8()? {
        0 => Some(&r.b[start..start]),
        1..=3 => {
            r.take(32)?;
            Some(&r.b[start..r.p])
        }
        _ => None,
    }
}

/// Parse a transaction; none if the bytes aren't exactly one.
pub fn parse(b: &[u8]) -> Option<Tx<'_>> {
    let mut r = Rd::new(b);
    r.take(4)?;
    let flag = r.u8()?;
    if flag > 1 {
        return None;
    }
    let body = r.p;
    let n = r.varint()?;
    let mut inputs = Vec::with_capacity(n);
    for _ in 0..n {
        let txid = r.array()?;
        let raw = r.u32()?;
        r.vbytes()?;
        r.u32()?;
        // the null prevout (a coinbase's) carries no flags
        let flagged = raw != u32::MAX;
        let issuance = flagged && raw & ISSUANCE != 0;
        if issuance {
            r.take(64)?;
            value(&mut r)?;
            value(&mut r)?;
        }
        let vout = if flagged { raw & INDEX } else { raw };
        inputs.push(Input { txid, vout, issuance, pegin: flagged && raw & PEGIN != 0 });
    }
    let n = r.varint()?;
    let mut outputs = Vec::with_capacity(n);
    for _ in 0..n {
        outputs.push(Output { asset: asset(&mut r)?, value: value(&mut r)?, nonce: nonce(&mut r)?, script: r.vbytes()? });
    }
    r.u32()?;
    let end = r.p;
    if flag == 1 {
        for _ in 0..inputs.len() {
            // issuance and inflation keys rangeproofs, script witness,
            // peg-in witness
            r.vbytes()?;
            r.vbytes()?;
            r.stack()?;
            r.stack()?;
        }
        for _ in 0..outputs.len() {
            // surjection proof, rangeproof
            r.vbytes()?;
            r.vbytes()?;
        }
    }
    if !r.done() {
        return None;
    }
    let mut bare = Vec::with_capacity(end);
    bare.extend_from_slice(&b[..4]);
    bare.push(0);
    bare.extend_from_slice(&b[body..end]);
    Some(Tx { inputs, outputs, txid: sha256d(&bare) })
}
