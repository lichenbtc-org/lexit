//! A Liquid block header (dynamic federations, "dynafed": every Liquid
//! block since its activation in 2021), its hash, and the federation's
//! signatures on it.
//!
//! ```text
//! version (4, bit 31 set) ‖ prev (32) ‖ merkle root (32) ‖ time (4) ‖ height (4)
//! ‖ current params ‖ proposed params          ← the block hash covers up to here
//! ‖ signblock witness (a stack)
//! ```
//!
//! Params are `00` (none), `01` compact (signblockscript, witness limit,
//! elided root: what a block's current params are) or `02` full
//! (signblockscript, witness limit, fedpeg program, fedpegscript,
//! extension space). A Liquid header is about 3.3 kB: its proposed params
//! are full, the fedpegscript and the PAK list in its extension space.

use alloc::vec::Vec;
use k256::ecdsa::signature::hazmat::PrehashVerifier;
use k256::ecdsa::{Signature, VerifyingKey};

use crate::read::Rd;
use crate::{sha256, sha256d, Hash};

/// A header's dynafed params.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Params<'a> {
    Null,
    Compact { signblockscript: &'a [u8], witness_limit: u32, elided_root: Hash },
    Full { signblockscript: &'a [u8], witness_limit: u32, fedpeg_program: &'a [u8], fedpegscript: &'a [u8], extension_space: Vec<&'a [u8]> },
}

impl<'a> Params<'a> {
    fn read(r: &mut Rd<'a>) -> Option<Params<'a>> {
        Some(match r.u8()? {
            0 => Params::Null,
            1 => Params::Compact { signblockscript: r.vbytes()?, witness_limit: r.u32()?, elided_root: r.array()? },
            2 => Params::Full { signblockscript: r.vbytes()?, witness_limit: r.u32()?, fedpeg_program: r.vbytes()?, fedpegscript: r.vbytes()?, extension_space: r.stack()? },
            _ => return None,
        })
    }
    /// The block signing script, if any.
    pub fn signblockscript(&self) -> Option<&'a [u8]> {
        match self {
            Params::Null => None,
            Params::Compact { signblockscript, .. } | Params::Full { signblockscript, .. } => Some(signblockscript),
        }
    }
}

/// A dynafed header, read from its full serialization (with its witness).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header<'a> {
    pub version: u32,
    pub prev: Hash,
    pub merkle_root: Hash,
    pub time: u32,
    pub height: u32,
    pub current: Params<'a>,
    pub proposed: Params<'a>,
    pub witness: Vec<&'a [u8]>,
    /// The serialization the block hash covers (without the witness).
    pub unsigned: &'a [u8],
}

/// The version bit that marks a dynafed header.
pub const DYNAFED: u32 = 1 << 31;

/// Parse a dynafed header (none for a legacy one, or bytes that aren't a
/// header).
pub fn parse(b: &[u8]) -> Option<Header<'_>> {
    let mut r = Rd::new(b);
    let version = r.u32()?;
    if version & DYNAFED == 0 {
        return None;
    }
    let prev = r.array()?;
    let merkle_root = r.array()?;
    let time = r.u32()?;
    let height = r.u32()?;
    let current = Params::read(&mut r)?;
    let proposed = Params::read(&mut r)?;
    let unsigned = &b[..r.p];
    let witness = r.stack()?;
    r.done().then_some(Header { version, prev, merkle_root, time, height, current, proposed, witness, unsigned })
}

impl Header<'_> {
    /// The block hash (internal order).
    pub fn hash(&self) -> Hash {
        sha256d(self.unsigned)
    }
}

/// A bare multisig script, `OP_m <key>… OP_n OP_CHECKMULTISIG`: `m` and
/// the keys (33 bytes each).
pub fn multisig(script: &[u8]) -> Option<(usize, Vec<&[u8]>)> {
    let small = |op: u8| (0x51..=0x60).contains(&op).then(|| usize::from(op - 0x50));
    let (&first, rest) = script.split_first()?;
    let m = small(first)?;
    let (&last, rest) = rest.split_last()?;
    if last != 0xae {
        return None;
    }
    let (&n_op, mut keys) = rest.split_last()?;
    let n = small(n_op)?;
    let mut out = Vec::with_capacity(n);
    while let Some((&len, rest)) = keys.split_first() {
        if len != 33 || rest.len() < 33 {
            return None;
        }
        out.push(&rest[..33]);
        keys = &rest[33..];
    }
    (out.len() == n && m <= n).then_some((m, out))
}

/// Whether the header is signed under `script`, the block signing script
/// a contract pins (the federation's: P2WSH of an `m`-of-`n` multisig):
/// its current params name that script, and its witness is the multisig's
/// (an empty dummy, `m` signatures in key order, the witness script),
/// each signature ECDSA (DER, `SIGHASH_ALL`) on the block hash.
pub fn signed(h: &Header, script: &[u8]) -> bool {
    if h.current.signblockscript() != Some(script) || script.len() != 34 || script[..2] != [0x00, 0x20] {
        return false;
    }
    let Some((&ws, items)) = h.witness.split_last() else { return false };
    if sha256(ws)[..] != script[2..] {
        return false;
    }
    let Some((m, keys)) = multisig(ws) else { return false };
    let Some((dummy, sigs)) = items.split_first() else { return false };
    if !dummy.is_empty() || sigs.len() != m {
        return false;
    }
    let msg = h.hash();
    // CHECKMULTISIG: each signature by a key after the last one's
    let mut k = 0;
    for s in sigs {
        let Some((&ht, der)) = s.split_last() else { return false };
        if ht != 0x01 {
            return false;
        }
        let Ok(sig) = Signature::from_der(der) else { return false };
        let sig = sig.normalize_s().unwrap_or(sig);
        loop {
            let Some(key) = keys.get(k) else { return false };
            k += 1;
            if VerifyingKey::from_sec1_bytes(key).is_ok_and(|vk| vk.verify_prehash(&msg, &sig).is_ok()) {
                break;
            }
        }
    }
    true
}
