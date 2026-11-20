//! The wallet's API, as the page calls it: served over HTTP on the
//! desktop (`lexit-gui`), and through a Tauri command in the app.

use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::{App, AskView, Conn};

/// The page, for both.
pub const PAGE: &str = include_str!("../ui/index.html");

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// `%xx` and `+` decoded (a QR's data).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let hex = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(v)) => {
                out.push(v);
                i += 2;
            }
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Deserialize)]
struct Cid {
    cid: u32,
}

pub fn route(app: &Arc<App>, method: &str, path: &str, query: &str, body: &str) -> Result<(String, &'static str)> {
    let j = |v: Value| Ok((v.to_string(), "application/json"));
    let body: Value = if body.trim().is_empty() { json!({}) } else { serde_json::from_str(body).context("a JSON body")? };
    match (method, path) {
        ("GET", "/api/state") => {
            let saved = app.saved.lock().unwrap().clone();
            let snap = app.snap.lock().unwrap().clone();
            let job = app.job.lock().unwrap().clone();
            let history = app.history.lock().unwrap().clone();
            let wallet = app.file.lock().unwrap().as_ref().map(|p| p.display().to_string());
            let launch = app.launch.lock().unwrap().clone();
            j(json!({ "saved": saved, "snap": snap, "job": job, "history": history, "pin": app.pending_pin(), "wallet": wallet, "dir": app.dir, "launch": launch, "version": { "app": env!("CARGO_PKG_VERSION"), "lexit": lexit_proto::VERSION } }))
        }
        ("POST", "/api/connect") => {
            // a preset's key, when published: a wallet made for its
            // deployment without asking
            let key: Option<String> = body.get("key").and_then(|k| k.as_str()).map(str::to_string);
            let conn: Conn = serde_json::from_value(body)?;
            *app.launch.lock().unwrap() = None;
            let mut pin = app.connect(conn)?;
            if let (Some(p), Some(key)) = (&pin, &key) {
                ensure!(p.hub == *key, "the hub's key isn't the one this app knows for it ({}): it isn't the hub it names, or it was redeployed", p.hub);
                app.pin()?;
                pin = None;
            }
            app.refresh();
            j(json!({ "pin": pin }))
        }
        ("GET", "/api/presets") => j(json!(crate::app::presets())),
        // the wallet's file, whole, wrapped with its network: a backup to
        // keep off the device (it holds the wallet's keys: its contracts'
        // and its L-BTC's mnemonic)
        ("GET", "/api/backup") => {
            let path = app.file.lock().unwrap().clone().context("no wallet: connect first")?;
            let wallet: Value = serde_json::from_slice(&std::fs::read(&path)?).context("the wallet's file")?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("wallet.json").to_string();
            let backup = json!({ "lexit_wallet_backup": 1, "network": app.conn().network, "name": name, "wallet": wallet });
            j(json!({ "name": format!("lexit-{name}"), "content": serde_json::to_string(&backup)? }))
        }
        // a backup put back: as the wallet for its deployment, if this
        // device has none (or has this very one)
        ("POST", "/api/restore") => {
            let content = body.get("content").and_then(Value::as_str).context("no backup given")?;
            let b: Value = serde_json::from_str(content).context("not a Lexit wallet backup (JSON)")?;
            ensure!(b.get("lexit_wallet_backup").is_some(), "not a Lexit wallet backup");
            let net: lichen_chain::Net = serde_json::from_value(b["network"].clone()).context("the backup's network")?;
            let wallet = &b["wallet"];
            let hub = wallet["l2"]["deployment"]["hub"].as_str().context("the backup's wallet names no hub key")?;
            let hub: lichen_proto::Hash = hex::decode(hub).ok().and_then(|h| h.try_into().ok()).context("the backup's hub key")?;
            ensure!(wallet["secret"].is_string() && wallet["l2"]["mnemonic"].is_string(), "the backup's wallet is incomplete");
            ensure!(!app.job.lock().unwrap().running, "an operation is running");
            let path = app.wallet_path(net, &hub);
            if path.exists() {
                let here: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                ensure!(here["secret"] == wallet["secret"], "this device has another wallet for that hub already: it isn't replaced (back it up, and remove it first, if you mean to)");
            }
            // the wallet in use is this one: dropped before its file is
            // replaced, and opened again from it (it would save over it)
            let in_use = app.file.lock().unwrap().as_ref() == Some(&path);
            if in_use {
                *app.wallet.lock().unwrap() = None;
            }
            lichen_proto::files::write_private(&path, &serde_json::to_vec_pretty(wallet)?)?;
            if in_use {
                app.connect(app.conn())?;
                app.refresh();
            }
            j(json!({ "network": net, "hub": hex::encode(hub), "path": path, "in_use": in_use }))
        }
        ("POST", "/api/pin") => {
            app.pin()?;
            app.refresh();
            j(json!({ "ok": true }))
        }
        ("GET", "/api/quote") => {
            let num = |k: &str| param(query, k).unwrap_or("0").parse::<u64>().with_context(|| format!("{k}: a number of sats"));
            let ask = lexit_wallet::Ask { deposit: num("deposit")?, liquidity: num("liquidity")?, reserve: num("reserve")? };
            let info = match app.snap.lock().unwrap().info.clone() {
                Some(i) => i,
                None => lexit_wallet::hub_info(&app.conn().hub)?,
            };
            let q = ask.quote(&info)?;
            let pay = lexit_wallet::LexitWallet::deposit_amount(&ask, info.network).to_sat();
            j(json!({ "fee": q.fee, "minted": q.minted, "cap": q.cap, "unit": q.unit, "inbound": q.cap.saturating_sub(q.minted), "pay": pay, "reserve": ask.reserve, "extra": pay - ask.deposit - ask.reserve }))
        }
        ("POST", "/api/open") => {
            #[derive(Deserialize)]
            struct Open {
                deposit: u64,
                liquidity: u64,
                reserve: u64,
                payout: String,
            }
            let o: Open = serde_json::from_value(body)?;
            app.open(AskView { deposit: o.deposit, liquidity: o.liquidity, reserve: o.reserve }, o.payout)?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/resume") => {
            #[derive(Deserialize)]
            struct Resume {
                cid: u32,
                ask: AskView,
            }
            let r: Resume = serde_json::from_value(body)?;
            app.resume(r.cid, r.ask)?;
            j(json!({ "ok": true }))
        }
        // L-BTC back to BTC: returned, then paid back by the hub
        ("POST", "/api/withdraw") => {
            #[derive(Deserialize)]
            struct W {
                cid: u32,
                amount: u64,
            }
            let w: W = serde_json::from_value(body)?;
            app.withdraw(w.cid, w.amount)?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/refund") => {
            let c: Cid = serde_json::from_value(body)?;
            app.spawn(&format!("refund the deposit for contract {}", c.cid), move |_, w, conn, _| {
                let txid = w.refund(&conn.chain()?, c.cid, bitcoin::Amount::from_sat(500))?;
                Ok(format!("refunded: {txid}"))
            })?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/sweep") => {
            let c: Cid = serde_json::from_value(body)?;
            app.spawn(&format!("sweep contract {}'s fee budget", c.cid), move |_, w, conn, _| {
                let chain = conn.chain()?;
                // closed by the hub's default, which the wallet didn't post:
                // its output spent, nothing left for the budget to pay
                let funding = w.contracts.iter().find(|h| h.record.cid() == c.cid).map(|h| h.record.funding).context("no such contract")?;
                let spent = !chain.unspent(&funding)?;
                let txid = w.sweep_fees(&chain, c.cid, bitcoin::Amount::from_sat(500), spent)?;
                Ok(format!("swept to the payout address: {txid}"))
            })?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/post-default") => {
            let c: Cid = serde_json::from_value(body)?;
            app.spawn(&format!("post contract {}'s default", c.cid), move |_, w, conn, _| {
                let info = lexit_wallet::hub_info(&conn.hub)?;
                let txid = w.post_default(&conn.chain()?, &crate::app::statement(&info)?, c.cid)?;
                Ok(format!("posted the default: {txid}, your reserve paid to the contract's payout address"))
            })?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/exit") => {
            let c: Cid = serde_json::from_value(body)?;
            app.spawn(&format!("exit contract {} on Bitcoin", c.cid), move |_, w, conn, _| {
                let info = lexit_wallet::hub_info(&conn.hub)?;
                let (outcome, swept) = w.exit(&conn.chain()?, &crate::app::statement(&info)?, c.cid)?;
                Ok(format!("the exit ended: {outcome:?}; {swept:?}"))
            })?;
            j(json!({ "ok": true }))
        }
        // paying from the payout key needs no wallet, so it runs beside an
        // operation (an opening waiting for this very payment)
        ("POST", "/api/send-payout") => {
            #[derive(Deserialize)]
            struct S {
                to: String,
                amount: Option<u64>,
            }
            let p: S = serde_json::from_value(body)?;
            let txid = app.send_payout(&p.to, p.amount)?;
            j(json!({ "txid": txid.to_string() }))
        }
        ("POST", "/api/pay-deposit") => {
            let pay = app.job.lock().unwrap().pay.clone().context("no deposit to pay: the opening isn't waiting for one")?;
            let txid = app.send_payout(&pay.address, Some(pay.sats))?;
            app.job.lock().unwrap().log.push(format!("paid the deposit from your payout address: {txid}"));
            j(json!({ "txid": txid.to_string() }))
        }
        // a mint the opening didn't sweep (Liquid unreachable then)
        ("POST", "/api/claim-mint") => {
            let c: Cid = serde_json::from_value(body)?;
            app.spawn(&format!("sweep contract {}'s mint", c.cid), move |_, w, _, _| {
                let txid = lexit_wallet::sweep_mint(w, c.cid)?;
                Ok(format!("the L-BTC is in your wallet: {txid}"))
            })?;
            j(json!({ "ok": true }))
        }
        // a plain L-BTC payment, to any Liquid address of this network
        ("POST", "/api/send") => {
            #[derive(Deserialize)]
            struct Send {
                to: String,
                amount: u64,
            }
            let s: Send = serde_json::from_value(body)?;
            let to: elements::Address = s.to.trim().parse().context("not a Liquid address")?;
            let ours: elements::Address = app.snap.lock().unwrap().address.parse().context("the wallet's L-BTC isn't synced yet: try again in a moment")?;
            ensure!(to.params == ours.params, "that address is for another Liquid network than this hub's ({})", app.snap.lock().unwrap().liquid);
            ensure!(s.amount > 0, "an amount to pay");
            let shown = to.to_string();
            app.spawn(&format!("pay {} sats of L-BTC", s.amount), move |_, w, _, _| {
                let txid = w.l2.lbtc_send(&to, s.amount)?;
                Ok(format!("paid {} sats to {}…: {txid}", s.amount, &shown[..16.min(shown.len())]))
            })?;
            j(json!({ "ok": true }))
        }
        ("POST", "/api/faucet") => {
            let conn = app.conn();
            ensure!(!conn.faucet.is_empty() && conn.network == lichen_chain::Net::Regtest, "no faucet: this is the devstack's, on regtest");
            let r: Value = ureq_post(&format!("{}/pay", conn.faucet), &body)?;
            j(r)
        }
        // L-BTC from the devstack's Elements node, to this wallet
        ("POST", "/api/faucet-lbtc") => {
            let conn = app.conn();
            ensure!(!conn.faucet.is_empty() && conn.network == lichen_chain::Net::Regtest, "no faucet: this is the devstack's, on regtest");
            let address = app.snap.lock().unwrap().address.clone();
            ensure!(!address.is_empty(), "the wallet's L-BTC isn't synced yet: try again in a moment");
            let sats = body.get("sats").and_then(Value::as_u64).unwrap_or(100_000);
            let r: Value = ureq_post(&format!("{}/pay-lbtc", conn.faucet), &json!({ "address": address, "sats": sats }))?;
            app.snap.lock().unwrap().lbtc_synced = 0;
            j(r)
        }
        ("GET", "/api/qr") => {
            let data = unescape(param(query, "d").unwrap_or_default());
            let code = qrcode::QrCode::new(data.as_bytes())?;
            let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(180, 180).quiet_zone(true).build();
            Ok((svg, "image/svg+xml"))
        }
        _ => bail!("no {method} {path}"),
    }
}

/// A call to the devstack's faucet.
fn ureq_post(url: &str, body: &Value) -> Result<Value> {
    match ureq::post(url).send_json(body) {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(_, r)) => bail!("the faucet: {}", r.into_string().unwrap_or_default()),
        Err(e) => Err(e.into()),
    }
}
