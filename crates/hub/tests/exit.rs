//! Exits on Bitcoin, on both regtest chains and the mock statement: the
//! hub refuses cooperative withdrawals, so each claim is an exit, decided
//! against Liquid.
//!
//! - E: an honest exit. Eve returns L-BTC and claims it: the hub finds no
//!   ground, and `request_timeout` pays her.
//! - N: not a return. Nate cites a real payment of L-BTC to his own
//!   Liquid wallet: the hub shows the block, its output paying another
//!   script, and asserts; his wallet, finding the proof valid, concedes.
//! - M: malformed. Mia returns L-BTC but claims a unit more than it is
//!   worth: the hub asserts the claim malformed.
//! - K: a lying hub. It asserts a false proof against Kim's honest claim;
//!   she challenges, it can't answer, and `failure` pays her.
//!
//! "Too high" (a cited block beyond Liquid's tip at the claim's time) is
//! the statement's tests' and M1's (`hp_inputs`): end to end it needs
//! Liquid's clock ahead of Bitcoin's, and regtest's fast blocks run
//! Bitcoin's block times ahead of the wall clock.

mod common;

use common::*;
use lexit_wallet::{ExitPlan, LexitWallet as Wallet};
use lichen_chain::Chain;
use lichen_wallet::dispute::Outcome;

const ASK: lexit_wallet::Ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };

/// A user of `hub` with a contract open: the wallet, its chain, the
/// contract.
fn open(env: &Env, hub: &Hub, name: &str) -> (Wallet, Chain, u32) {
    let (mut w, ch) = env.user(name, hub);
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let held = env.mining(|| w.open_contract(&hub.url, &ch, &statement(), ASK, ch.new_spk().unwrap(), &pay)).unwrap();
    let cid = held.record.cid();
    (w, ch, cid)
}

/// A return of `sats` paid and recorded.
fn returned(env: &Env, w: &mut Wallet, name: &str, cid: u32, sats: u64) {
    let liquid = env.liquid_wallet(&format!("{name}-liquid"), 1_000_000);
    let address = lexit_wallet::return_address(w, cid).unwrap().to_string();
    let r = env.pay(&liquid, cid, &address, sats);
    w.l2.record_return(r).unwrap();
}

fn exit(env: &Env, w: &mut Wallet, ch: &Chain, name: &str, cid: u32) -> Outcome {
    let t = std::time::Instant::now();
    let (o, swept) = env.mining(|| w.exit(ch, &statement(), cid)).unwrap();
    println!("LEXIT {name}: {o:?} in {:.1?}; swept {swept:?}", t.elapsed());
    o
}

#[test]
fn exits_are_decided_against_liquid() {
    let env = Env::new("exit");
    let hub = env.hub("hub", |c| c.refuse_folds = true);

    // E: an honest exit
    let (mut w, ch, cid) = open(&env, &hub, "eve");
    returned(&env, &mut w, "eve", cid, 150_000);
    assert!(matches!(exit(&env, &mut w, &ch, "eve", cid), Outcome::HubTimedOut(_)), "E: the honest exit is paid");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: the user's exit claim holds"))), "E: the hub found no ground");

    // N: a payment that isn't a return
    let (mut w, ch, cid) = open(&env, &hub, "nate");
    let liquid = env.liquid_wallet("nate-liquid", 1_000_000);
    let own = liquid.wallet_call("getnewaddress", serde_json::json!([])).unwrap();
    let r = env.pay(&liquid, cid, own.as_str().unwrap(), 150_000);
    w.l2.record_return(r).unwrap();
    assert_eq!(exit(&env, &mut w, &ch, "nate", cid), Outcome::Lost, "N: the hub proves it no return");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: the user's exit claim is false: block"))), "N: on the block's ground");

    // M: a unit more than the returns are worth
    let (mut w, ch, cid) = open(&env, &hub, "mia");
    returned(&env, &mut w, "mia", cid, 150_000);
    let k = w.contracts.iter().position(|h| h.record.cid() == cid).unwrap();
    let plan: ExitPlan = serde_json::from_value(<lexit_wallet::LiquidWallet as lichen_wallet::WalletL2>::exit_plan(&mut w, &ch, cid).unwrap()).unwrap();
    let claim = lexit_statement::Claim::from_bytes(&plan.posted().unwrap()).unwrap();
    let deposit = lexit_proto::statement_outpoint(&w.contracts[k].record.dispute.deposit);
    let b = plan.b + 1;
    let greedy = ExitPlan { b, e: hex::encode(claim.fingerprint(cid, &deposit, b)), posted: plan.posted.clone() };
    w.contracts[k].exit = Some(serde_json::to_value(&greedy).unwrap());
    assert_eq!(exit(&env, &mut w, &ch, "mia", cid), Outcome::Lost, "M: the hub proves it malformed");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}: the user's exit claim is false: the claim is malformed"))));

    // K: a lying hub
    let liar = env.hub("liar", |c| {
        c.refuse_folds = true;
        c.forge_assertions = true;
    });
    let (mut w, ch, cid) = open(&env, &liar, "kim");
    returned(&env, &mut w, "kim", cid, 150_000);
    assert!(matches!(exit(&env, &mut w, &ch, "kim", cid), Outcome::Refuted(_)), "K: the false assertion refuted, failure pays her");
}
