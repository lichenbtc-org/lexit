//! A contract's life, cooperatively, on both regtest chains: Alice opens
//! (her deposit on Bitcoin, the hub's mint of L-BTC on Liquid, one atomic
//! swap: her wallet checks the mint on Liquid before signing, and once the
//! funding shows `t` sweeps it into its own L-BTC, an LWK wallet); she
//! returns L-BTC from it to her contract's return address, her wallet
//! opening and recording the return; the hub checks it on Liquid and
//! folds: BTC for her L-BTC.

mod common;

use common::*;
use lexit_proto::liquid::LiquidView;

#[test]
fn a_contract_opens_with_a_mint_and_folds_against_a_return() {
    let env = Env::new("open-fold");
    let hub = env.hub("hub", |_| {});
    let (mut w, ch) = env.user("alice", &hub);
    let ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let held = env.mining(|| w.open_contract(&hub.url, &ch, &statement(), ask, ch.new_spk().unwrap(), &pay)).unwrap();
    let cid = held.record.cid();
    let mint = w.l2.mints.iter().find(|m| m.cid == cid).expect("the mint's key kept").clone();
    assert_eq!(mint.value, held.record.minted());
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: minted"))));
    assert!(mint.swept.is_some(), "the mint swept at the opening");
    let mut balance = 0;
    wait("the swept mint in her L-BTC", || {
        balance = w.l2.lbtc_balance().unwrap();
        balance > 0
    });
    assert!(balance <= mint.value && balance > mint.value - 1_000, "the mint, less the sweep's fee: {balance}");

    // a return, from her L-BTC: opened and recorded by her wallet
    let r = lexit_wallet::pay_return(&mut w, cid, 150_000).unwrap();
    assert_eq!(r.value, 150_000);
    wait("the return confirmed", || env.el.rpc.tx_block(&internal(&r.txid)).unwrap().is_some());

    let fold = env.mining(|| w.withdraw(&ch, &hub.url, &statement(), cid, 150_000)).unwrap();
    println!("LEXIT alice: the fold {fold}");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: the user's returns"))), "the hub checked the return on Liquid");
    wait("the contract closed", || w.contracts.iter().any(|h| h.record.cid() == cid && h.closed.is_some()));
}
