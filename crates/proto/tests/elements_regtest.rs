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

