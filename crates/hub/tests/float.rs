//! A hub without the L-BTC to mint refuses an opening at its setup, before
//! any work on it (the user keeps her deposit, to refund or to resume), and
//! says so in its float (`GET /liquid/float`).

mod common;

use common::*;

#[test]
fn a_hub_without_the_l_btc_refuses_the_opening() {
    let env = Env::new("float");
    let hub = env.hub_with("poor", 0, |_| {});
    let float: serde_json::Value = lexit_wallet::hub_call(&format!("{}/liquid/float", hub.url), None, true).unwrap();
    assert_eq!(float["balance"].as_u64(), Some(0), "{float}");
    assert!(float["address"].as_str().is_some_and(|a| !a.is_empty()));

    let (mut w, ch) = env.user("ivy", &hub);
    let ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let refused = env.mining(|| w.open_contract(&hub.url, &ch, &statement(), ask, ch.new_spk().unwrap(), &pay));
    let Err(e) = refused else { panic!("the opening went through") };
    let e = format!("{e:#}");
    assert!(e.contains("can't mint this contract now"), "{e}");
    assert!(!hub.log().iter().any(|l| l.contains("minted")), "no mint");
}
