//! What the hub's statement reads of Liquid (an Elements chain), with no
//! dependency the RISC Zero guest can't build (`no_std`; SHA-256 and
//! secp256k1 by RustCrypto, which RISC Zero accelerates):
//!
//! - [`header`]: a dynafed block header, its hash, its current block
//!   signing script, and the federation's signatures in its witness
//!   (`CHECKMULTISIG` over the block hash, [`header::signed`]);
//! - [`tx`]: a transaction, its txid, and its outputs: asset, value and
//!   nonce, each explicit or confidential, and the script;
//! - [`merkle`]: the block's transaction tree (Bitcoin's);
//! - [`commit`]: an output's asset and value commitments opened with their
//!   blinding factors, against an asset's generator.
//!
//! Hashes are in internal byte order, as hashed (a txid, block hash or
//! asset id displayed is this reversed).

#![no_std]

extern crate alloc;

pub mod commit;
pub mod header;
pub mod merkle;
pub mod tx;

mod read;

use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub fn sha256(data: &[u8]) -> Hash {
    Sha256::digest(data).into()
}

pub fn sha256d(data: &[u8]) -> Hash {
    Sha256::digest(Sha256::digest(data)).into()
}

/// Liquid's policy asset, L-BTC: its id in internal byte order (displayed
/// `6f0279e9ed041c3d710a9f57d0c02928416460c4b722ae3457a11eec381c526d`).
pub const LBTC: Hash = [
    0x6d, 0x52, 0x1c, 0x38, 0xec, 0x1e, 0xa1, 0x57, 0x34, 0xae, 0x22, 0xb7, 0xc4, 0x60, 0x64, 0x41, 0x28, 0x29, 0xc0, 0xd0, 0x57, 0x9f, 0x0a, 0x71, 0x3d, 0x1c, 0x04, 0xed, 0xe9, 0x79, 0x02, 0x6f,
];
