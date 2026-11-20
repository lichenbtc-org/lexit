//! Opt-in (`--ignored`): the hub's statement for real, RISC Zero's proof
//! wrapped in Groth16, on both regtest chains. Needs the hub's prover:
//! LICHEN_HP_PROVER, r0/hp-split.sh from a Mac (the STARK here, by r0's
//! release build of lexit-r0; the wrap on LICHEN_R0_HOST, its tree
//! LICHEN_R0_TREE, the prover image built there), or r0/hp-prover.sh on an
//! x86 Linux box with the image.
//!
//! Mal opens a contract on `hub-proves` (she garbles under RISC Zero's
//! key), returns L-BTC, and claims a unit more than it is worth. The hub
//! proves the claim malformed and asserts; her wallet finds the proof
//! valid and concedes; `success` is the hub's.
//!
//! Bitcoin's blocks come every 150 ms, but one a minute while the hub
//! proves (its files in `hp/<cid>/`: the prover's log and no seal yet), so
//! that `w_H` covers the proof.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitcoincore_rpc::{Auth, RpcApi};
use common::*;
use lexit_proto::liquid::LiquidView;
use lexit_wallet::ExitPlan;
use lichen_proto::Statement;
use lichen_wallet::dispute::Outcome;

/// Whether a prover runs in `hp`: a contract's prover's log without its
/// seal.
fn proving(hp: &std::path::Path) -> bool {
    let Ok(dirs) = std::fs::read_dir(hp) else { return false };
    dirs.flatten().filter(|d| d.path().is_dir()).any(|d| d.path().join("prover.log").exists() && !d.path().join("proof.seal").exists())
}

#[test]
#[ignore]
fn a_malformed_claim_proved_false_for_real() {
    assert!(std::env::var("LICHEN_HP_PROVER").is_ok(), "set LICHEN_HP_PROVER (r0/hp-split.sh, or r0/hp-prover.sh)");
    let statement = Statement::parse("hub-proves", lichen_babe::statement::image_id(lexit_proto::HP_IMAGE_ID)).unwrap();
    let env = Env::new("hp-real");
    let hub = env.hub("hub", |c| {
        c.statement = statement.clone();
        c.refuse_folds = true;
    });
    let hp_dir = env.dir.join("hub").join("hp");
    let stop = Arc::new(AtomicBool::new(false));
    let miner = {
        let rpc = bitcoincore_rpc::Client::new(&format!("{}/wallet/harness", env.rt.rpc_url()), Auth::CookieFile(env.rt.cookie().clone())).unwrap();
        let addr = rpc.get_new_address(None, None).unwrap().assume_checked();
        let (s, hp) = (stop.clone(), hp_dir.clone());
        std::thread::spawn(move || {
            let mut last = Instant::now();
            while !s.load(Ordering::Relaxed) {
                let every = if proving(&hp) { Duration::from_secs(60) } else { Duration::from_millis(150) };
                if last.elapsed() >= every {
                    let _ = rpc.generate_to_address(1, &addr);
                    last = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };

    let (mut w, ch) = env.user("mal", &hub);
    let ask = lexit_wallet::Ask { deposit: 200_000, liquidity: 1_000_000, reserve: 20_000 };
    let t = Instant::now();
    let pay = |addr: &bitcoin::Address, v: bitcoin::Amount| ch.pay(&addr.script_pubkey(), v).map(|_| ());
    let cid = w.open_contract(&hub.url, &ch, &statement, ask, ch.new_spk().unwrap(), &pay).unwrap().record.cid();
    println!("LEXIT mal: contract {cid} opened on the real statement in {:.1?}", t.elapsed());
    let r = lexit_wallet::pay_return(&mut w, cid, 150_000).unwrap();
    wait("the return confirmed", || env.el.rpc.tx_block(&internal(&r.txid)).unwrap().is_some());

    // a unit more than the return is worth: malformed
    let k = w.contracts.iter().position(|h| h.record.cid() == cid).unwrap();
    let plan: ExitPlan = serde_json::from_value(<lexit_wallet::LiquidWallet as lichen_wallet::WalletL2>::exit_plan(&mut w, &ch, cid).unwrap()).unwrap();
    let claim = lexit_statement::Claim::from_bytes(&plan.posted().unwrap()).unwrap();
    let deposit = lexit_proto::statement_outpoint(&w.contracts[k].record.dispute.deposit);
    let b = plan.b + 1;
    let greedy = ExitPlan { b, e: hex::encode(claim.fingerprint(cid, &deposit, b)), posted: plan.posted.clone() };
    w.contracts[k].exit = Some(serde_json::to_value(&greedy).unwrap());

    let t = Instant::now();
    let (o, _) = w.exit(&ch, &statement, cid).unwrap();
    println!("LEXIT mal: {o:?} in {:.1?}", t.elapsed());
    for l in hub.log().iter().filter(|l| l.contains(&format!("contract {cid}:"))) {
        println!("LEXIT hub: {l}");
    }
    for l in std::fs::read_to_string(hp_dir.join(cid.to_string()).join("prover.log")).unwrap_or_default().lines().filter(|l| l.contains("lexit-r0")) {
        println!("LEXIT hub's prover: {l}");
    }
    assert_eq!(o, Outcome::Lost, "the hub proves the malformed claim false, and she concedes");
    assert!(hub.log().iter().any(|l| l.contains(&format!("contract {cid}:")) && l.contains("proved")), "with a real proof");
    wait("success the hub's", || hub.contracts().iter().any(|(r, closed)| r.cid() == cid && closed.is_some()));
    stop.store(true, Ordering::Relaxed);
    miner.join().unwrap();
}
