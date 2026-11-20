//! A mint whose opening is abandoned goes back to the hub, by its refund
//! leaf, after its timeout; a mint whose contract opened is the user's and
//! is never touched.
//!
//! Ann opens a contract (her mint swept into her wallet). Then the hub
//! mints Bea a sat short: her wallet refuses the offer, and the mint is
//! left at its address. Before the timeout `POST /liquid/refund` refunds
//! nothing; after it, Bea's mint and not Ann's.

mod common;

use common::*;
use lexit_hub::OpeningFault;

fn refund(hub: &Hub) -> u64 {
    let r: serde_json::Value = lexit_wallet::hub_call(&format!("{}/liquid/refund", hub.url), Some(String::new()), true).unwrap();
    r["refunded"].as_u64().unwrap()
}

#[test]
fn an_abandoned_opening_s_mint_is_refunded_after_its_timeout() {
    let env = Env::new("refund");
    let hub = env.hub("hub", |_| {});
    let ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };

    let (mut ann, ch) = env.user("ann", &hub);
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let held = env.mining(|| ann.open_contract(&hub.url, &ch, &statement(), ask, ch.new_spk().unwrap(), &pay)).unwrap();
    let ann_cid = held.record.cid();

    hub.set_opening_fault(OpeningFault::ShortMint);
    let (mut bea, ch) = env.user("bea", &hub);
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let refused = env.mining(|| bea.open_contract(&hub.url, &ch, &statement(), ask, ch.new_spk().unwrap(), &pay));
    assert!(refused.is_err(), "Bea's wallet refuses the short mint");
    hub.set_opening_fault(OpeningFault::None);

    assert_eq!(refund(&hub), 0, "nothing before the timeout");
    env.el.mine(u32::from(lexit_proto::LiquidNet::Regtest.mint_timeout())).unwrap();
    assert_eq!(refund(&hub), 1, "Bea's mint, not Ann's");
    assert!(hub.log().iter().any(|l| l.contains("its opening abandoned, the mint refunded")));
    assert!(!hub.log().iter().any(|l| l.contains(&format!("contract {ann_cid}: its opening abandoned"))), "Ann's contract opened: her mint is hers");
    assert_eq!(refund(&hub), 0, "each refunded once");
}
