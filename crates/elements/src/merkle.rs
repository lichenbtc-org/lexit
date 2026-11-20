//! The block's transaction tree: Bitcoin's (double SHA-256 of each pair,
//! a level's odd last node paired with itself).
//!
//! The statement shows a transaction by the block's every txid (Liquid
//! blocks are small: a handful of transactions), which shows its index in
//! range or out of it alike, and leaves no path's length to get wrong
//! (CVE-2017-12842). A level that pairs two equal nodes is refused, as
//! Bitcoin Core refuses it: a list with its last txids repeated reaches
//! the same root (CVE-2012-2459).

use alloc::vec::Vec;

use crate::{sha256d, Hash};

/// The root of `txids`; none if empty, or if a level pairs two equal
/// nodes.
pub fn root(txids: &[Hash]) -> Option<Hash> {
    if txids.is_empty() {
        return None;
    }
    let mut level: Vec<Hash> = txids.to_vec();
    while level.len() > 1 {
        let n = level.len();
        if level.chunks_exact(2).any(|p| p[0] == p[1]) {
            return None;
        }
        if n % 2 == 1 {
            level.push(level[n - 1]);
        }
        level = level
            .chunks(2)
            .map(|p| {
                let mut b = [0u8; 64];
                b[..32].copy_from_slice(&p[0]);
                b[32..].copy_from_slice(&p[1]);
                sha256d(&b)
            })
            .collect();
    }
    Some(level[0])
}
