//! The Elements regtest chain (`lexit_proto::regtest`) is Liquid-shaped:
//! its blocks are dynafed, signed by our federation as the statement checks
//! a Liquid block, and its wallet holds the free coins. Needs `elementsd`
//! (`LEXIT_ELEMENTSD`).

use lexit_elements::header;
use lexit_proto::liquid::LiquidView;
use lexit_proto::regtest::{ElementsRegtest, Federation};

#[test]
fn our_federation_signs_dynafed_blocks_the_statement_accepts() {
    let fed = Federation::regtest();
    let node = ElementsRegtest::start(fed.clone()).unwrap();
    node.mine(3).unwrap();
    let rpc = &node.rpc;
    assert_eq!(rpc.height().unwrap(), 4);
    for h in 1..=4 {
        let b = rpc.header(&rpc.block_hash(h).unwrap()).unwrap();
        let hd = header::parse(&b).expect("a dynafed header");
        assert_eq!(hd.height, h);
        assert!(header::signed(&hd, &fed.signblockscript()), "block {h} signed by our federation, as the statement checks");
        assert!(!header::signed(&hd, &Federation { m: 2, keys: Federation::regtest().keys.into_iter().rev().collect() }.signblockscript()));
    }
    assert_eq!(lexit_proto::liquid::signblockscript(rpc).unwrap(), fed.signblockscript());
    let balance = rpc.wallet_call("getbalance", serde_json::json!([])).unwrap();
    assert!(balance["bitcoin"].as_f64().unwrap() > 1000.0, "the free coins are the harness's: {balance}");
}


/// The hub takes back a mint whose funding never came: refused before the
/// mint's timeout (its refund leaf's CSV), accepted after.
#[test]
fn an_abandoned_mint_is_refunded_by_its_leaf_after_the_timeout() {
    use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
    use lexit_proto::liquid::{self, LiquidNet};

    let node = ElementsRegtest::start(Federation::regtest()).unwrap();
    let rpc = &node.rpc;
    let secp = Secp256k1::new();
    let key = |n: u8| SecretKey::from_slice(&[n; 32]).unwrap();
    // her contract's L2 key and blinding key, the funding's adaptor point,
    // the hub's key
    let l2 = key(1).x_only_public_key(&secp).0.serialize();
    let blinding = key(2);
    let point = PublicKey::from_secret_key(&secp, &key(3));
    let hub = key(4);
    let hub_x = hub.x_only_public_key(&secp).0.serialize();
    let address = liquid::mint_address(&l2, &point, &hub_x, &PublicKey::from_secret_key(&secp, &blinding), LiquidNet::Regtest).unwrap();
    let txid = rpc.wallet_call("sendtoaddress", serde_json::json!([address.to_string(), "0.002"])).unwrap();
    node.mine(1).unwrap();
    let mut id: [u8; 32] = hex::decode(txid.as_str().unwrap()).unwrap().try_into().unwrap();
    id.reverse();
    let tx: elements::Transaction = elements::encode::deserialize(&rpc.tx(&id).unwrap()).unwrap();
    let vout = tx.output.iter().position(|o| o.script_pubkey == address.script_pubkey()).unwrap() as u32;
    // the hub's node wallet made the payment; here her blinding key opens it
    let zkp = elements::secp256k1_zkp::Secp256k1::new();
    let secrets = tx.output[vout as usize].unblind(&zkp, elements::secp256k1_zkp::SecretKey::from_slice(&blinding.secret_bytes()).unwrap()).unwrap();
    let to: elements::Address = rpc.wallet_call("getnewaddress", serde_json::json!([])).unwrap().as_str().unwrap().parse().unwrap();
    let genesis = rpc.block_hash(0).unwrap().parse().unwrap();
    let refund = liquid::refund_tx(&tx, vout, secrets, &l2, &point, &hub, LiquidNet::Regtest, &to, 500, genesis).unwrap();
    let hex = hex::encode(elements::encode::serialize(&refund));
    let early = rpc.call("sendrawtransaction", serde_json::json!([hex]));
    assert!(early.as_ref().is_err_and(|e| e.to_string().contains("non-BIP68-final")), "refused before the timeout: {early:?}");
    node.mine(u32::from(LiquidNet::Regtest.mint_timeout())).unwrap();
    let sent = rpc.call("sendrawtransaction", serde_json::json!([hex])).unwrap();
    assert_eq!(sent.as_str().unwrap(), refund.txid().to_string());
    node.mine(1).unwrap();
    assert!(rpc.tx_block(&refund.txid().to_byte_array()).unwrap().is_some(), "the refund confirmed");
}
