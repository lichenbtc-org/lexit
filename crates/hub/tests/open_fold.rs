//! A contract's life, cooperatively, on both regtest chains: Alice opens
//! (her deposit on Bitcoin, the hub's mint of L-BTC on Liquid, one atomic
//! swap: her wallet checks the mint on Liquid before signing, and keeps its
//! key once the funding shows `t`); she returns L-BTC to her contract's
//! return address from another Liquid wallet; the hub checks the return on
//! Liquid and folds: BTC for her L-BTC.

mod common;

use common::*;

#[test]
fn a_contract_opens_with_a_mint_and_folds_against_a_return() {
    let env = Env::new("open-fold");
    let hub = env.hub("hub", |_| {});
    let (mut w, ch) = env.user("alice", &hub);
    let ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let held = env.mining(|| w.open_contract(&hub.url, &ch, &statement(), ask, ch.new_spk().unwrap(), &pay)).unwrap();
    let cid = held.record.cid();
    let mint = w.l2.mints.iter().find(|m| m.cid == cid).expect("the mint's key kept");
    assert_eq!(mint.value, held.record.minted());
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: minted"))));

    // a return, from a Liquid wallet of hers
    let liquid = env.liquid_wallet("alice-liquid", 1_000_000);
    let address = lexit_wallet::return_address(&w, cid).unwrap().to_string();
    let r = env.pay(&liquid, cid, &address, 150_000);
    w.l2.record_return(r).unwrap();

    let fold = env.mining(|| w.withdraw(&ch, &hub.url, &statement(), cid, 150_000)).unwrap();
    println!("LEXIT alice: the fold {fold}");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: the user's returns"))), "the hub checked the return on Liquid");
    wait("the contract closed", || w.contracts.iter().any(|h| h.record.cid() == cid && h.closed.is_some()));
}
