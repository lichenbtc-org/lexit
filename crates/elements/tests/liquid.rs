//! Against Liquid itself: block 4,153,000 (its header and both its
//! transactions, from Blockstream's Esplora), and secp256k1-zkp's
//! generators and commitments (by rust-elements).

use elements::secp256k1_zkp::{Generator, PedersenCommitment, Secp256k1, Tweak};
use liquid_lichen_elements::commit::{blinded_generator, opens, value_commitment, LBTC_GENERATOR};
use liquid_lichen_elements::tx::{Asset, Value};
use liquid_lichen_elements::{header, merkle, tx, LBTC};

fn data(name: &str) -> Vec<u8> {
    hex::decode(std::fs::read_to_string(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap().trim()).unwrap()
}

/// A hash as displayed (reversed) into internal order.
fn internal(display: &str) -> [u8; 32] {
    let mut h: [u8; 32] = hex::decode(display).unwrap().try_into().unwrap();
    h.reverse();
    h
}

const BLOCK: &str = "d01b2b352e1f2c7dde2c8a86bd75f5a0f6dc4a55f5bd1f007f816f6d88ad3229";
const COINBASE: &str = "d57aa81e99ea4961a80a880d49cf83766966a30539474fe26fe814b5b52e87e1";
const TX1: &str = "5a668457a3f255aa15f32906814a29333b0b977c2a77c1cd18a9a057d6f97fad";
/// Liquid's block signing script: P2WSH of the federation's 11-of-15.
const SIGNBLOCKSCRIPT: &str = "00207f1a37f651f3cdae15be58ecca344c4c424d3ac250e00452bdb16a9777e6be74";

#[test]
fn a_liquid_header_parses_hashes_and_is_signed_by_the_federation() {
    let b = data("block-4153000.header.hex");
    let h = header::parse(&b).expect("a dynafed header");
    assert_eq!(h.height, 4_153_000);
    assert_eq!(h.time, 1_795_149_550);
    assert_eq!(h.hash(), internal(BLOCK));
    let script = hex::decode(SIGNBLOCKSCRIPT).unwrap();
    assert!(matches!(h.current, header::Params::Compact { .. }));
    let (m, keys) = header::multisig(h.witness.last().unwrap()).unwrap();
    assert_eq!((m, keys.len()), (11, 15));
    assert!(header::signed(&h, &script), "the federation's signatures on the block hash");
    // another script, or a signature changed, and it isn't
    let mut other = script.clone();
    other[5] ^= 1;
    assert!(!header::signed(&h, &other));
    let mut b2 = b.clone();
    let at = b.len() - 600; // inside a signature
    b2[at] ^= 1;
    let h2 = header::parse(&b2).unwrap();
    assert!(!header::signed(&h2, &script));
    // the time changed: another hash, the signatures don't hold
    let mut b3 = b.clone();
    b3[68] ^= 1;
    assert!(!header::signed(&header::parse(&b3).unwrap(), &script));
}

#[test]
fn liquid_transactions_parse_to_their_txids_and_the_blocks_root() {
    let cb = data("tx-coinbase-4153000.hex");
    let t1 = data("tx-1-4153000.hex");
    let cb = tx::parse(&cb).expect("the coinbase");
    let t1 = tx::parse(&t1).expect("a confidential transaction");
    assert_eq!(cb.txid, internal(COINBASE));
    assert_eq!(t1.txid, internal(TX1));
    let b = data("block-4153000.header.hex");
    let h = header::parse(&b).unwrap();
    assert_eq!(merkle::root(&[cb.txid, t1.txid]), Some(h.merkle_root));
    assert_eq!(merkle::root(&[t1.txid, cb.txid]).map(|r| r == h.merkle_root), Some(false));
    // four confidential outputs and the fee, explicit L-BTC
    assert_eq!(t1.outputs.len(), 5);
    for o in &t1.outputs[..4] {
        assert!(matches!(o.asset, Asset::Confidential(_)) && matches!(o.value, Value::Confidential(_)));
        assert_eq!(o.nonce.len(), 33);
    }
    let fee = &t1.outputs[4];
    assert_eq!((fee.asset, fee.value, fee.script.len()), (Asset::Explicit(LBTC), Value::Explicit(46), 0));
}

#[test]
fn the_merkle_root_refuses_a_repeated_pair() {
    let (a, b, c) = ([1; 32], [2; 32], [3; 32]);
    let r = merkle::root(&[a, b, c]).unwrap();
    assert_eq!(merkle::root(&[a, b, c, c]), None, "the same root as [a, b, c], mutated");
    assert_ne!(merkle::root(&[a, b]), Some(r));
}

#[test]
fn lbtc_s_generator_is_secp256k1_zkp_s() {
    let secp = Secp256k1::new();
    let g = Generator::new_unblinded(&secp, elements::AssetId::LIQUID_BTC.into_tag());
    assert_eq!(g.serialize(), LBTC_GENERATOR, "LBTC_GENERATOR: {}", hex::encode(g.serialize()));
    let mut id = LBTC;
    id.reverse();
    assert_eq!(hex::encode(id), elements::AssetId::LIQUID_BTC.to_string());
}

#[test]
fn commitments_open_as_secp256k1_zkp_makes_them() {
    let secp = Secp256k1::new();
    let tag = elements::AssetId::LIQUID_BTC.into_tag();
    for (k, v) in [(1u8, 0u64), (2, 1), (3, 46), (4, 123_456_789), (5, 2_100_000_000_000_000)] {
        let r_a = [k; 32];
        let r_v = [k.wrapping_mul(7).wrapping_add(1); 32];
        let a = Generator::new_blinded(&secp, tag, Tweak::from_slice(&r_a).unwrap());
        let c = PedersenCommitment::new(&secp, v, Tweak::from_slice(&r_v).unwrap(), a);
        assert_eq!(blinded_generator(&LBTC_GENERATOR, &r_a), Some(a.serialize()));
        assert_eq!(value_commitment(&a.serialize(), v, &r_v), Some(c.serialize()));
        assert!(opens(&a.serialize(), &c.serialize(), &LBTC_GENERATOR, v, &r_a, &r_v));
        assert!(!opens(&a.serialize(), &c.serialize(), &LBTC_GENERATOR, v + 1, &r_a, &r_v));
        assert!(!opens(&a.serialize(), &c.serialize(), &LBTC_GENERATOR, v, &r_v, &r_v));
    }
}
