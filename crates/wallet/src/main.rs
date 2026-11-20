//! `lexit`: the user's wallet (a lichen wallet whose contracts are
//! guaranteed exits from L-BTC).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lichen_proto::Statement;
use lexit_wallet::{LexitWallet as Wallet, LiquidWallet, Return};

#[derive(Parser)]
#[command(about = "A Lexit wallet")]
struct Args {
    /// The Bitcoin network: `regtest`, `mutinynet`, `signet` or `testnet4`.
    #[arg(long, default_value = "regtest")]
    network: lichen_proto::Net,
    /// The wallet file.
    #[arg(long, default_value = "wallet.json")]
    wallet: PathBuf,
    /// The hub.
    #[arg(long, default_value = "http://127.0.0.1:18330")]
    hub: String,
    /// Bitcoin Esplora servers, comma-separated (the network's public one
    /// by default).
    #[arg(long)]
    esplora: Option<String>,
    /// Instead of Esplora: a Bitcoin node's RPC URL and cookie file.
    #[arg(long)]
    rpc_url: Option<String>,
    #[arg(long)]
    cookie: Option<PathBuf>,
    #[arg(long, default_value = "alice")]
    chain_wallet: String,
    /// Liquid's Esplora (the Liquid network's public one by default; a
    /// regtest chain needs its own).
    #[arg(long)]
    esplora_liquid: Option<String>,
    /// The hub's statement: `hub-proves` or `hub-proves-mock`.
    #[arg(long, default_value = "hub-proves-mock")]
    statement: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a wallet with a fresh key, for the hub.
    Init,
    /// Open a contract: pay `deposit` sats of BTC in, be paid L-BTC on
    /// Liquid (less the hub's fee), with a cap of the deposit and the
    /// liquidity.
    Open {
        deposit: u64,
        #[arg(long, default_value_t = 0)]
        liquidity: u64,
        #[arg(long, default_value_t = 20_000)]
        reserve: u64,
        #[arg(long)]
        payout: Option<String>,
        #[arg(long)]
        resume: Option<u32>,
    },
    /// The contracts, their mints and their return addresses.
    Contracts,
    /// Contract `contract`'s return address on Liquid: L-BTC paid to it is
    /// paid back in BTC from the contract, up to its cap.
    ReturnAddress { contract: u32 },
    /// Record a return you paid (from any Liquid wallet) for `contract`:
    /// its output, value in sats, and blinding factors (hex; zeros for an
    /// explicit output).
    RecordReturn { contract: u32, txid: String, vout: u32, value: u64, r_a: String, r_v: String },
    /// Withdraw `amount` sats of BTC from `contract` (cooperatively: the
    /// fold), citing your returns.
    Withdraw {
        contract: u32,
        amount: u64,
        /// If the hub doesn't fold, exit on Bitcoin.
        #[arg(long)]
        escalate: bool,
    },
    /// Exit on Bitcoin: claim your returns; the hub must prove the claim
    /// false or its timeout pays you.
    Exit { contract: u32 },
    PostDefault { contract: u32 },
    Refund {
        contract: u32,
        #[arg(long, default_value_t = 500)]
        fee: u64,
    },
    SweepFees {
        contract: u32,
        #[arg(long, default_value_t = 500)]
        fee: u64,
        #[arg(long)]
        force: bool,
    },
    /// The payout address and its balance.
    Payout,
    SendPayout { to: String, amount: Option<u64> },
}

fn statement(a: &Args) -> Result<Statement> {
    Statement::parse(&a.statement, lichen_babe::statement::image_id(lexit_proto::HP_IMAGE_ID))
}

fn chain(a: &Args) -> Result<lichen_chain::Chain> {
    match (&a.rpc_url, &a.cookie) {
        (Some(url), Some(cookie)) => lichen_chain::Chain::connect_on(url, cookie, &a.chain_wallet, a.network),
        (Some(_), None) => anyhow::bail!("--rpc-url needs --cookie"),
        _ => {
            let urls = a.esplora.as_deref().or(a.network.default_esplora()).context("--esplora: no public Esplora for this network")?;
            let urls: Vec<&str> = urls.split(',').map(str::trim).filter(|u| !u.is_empty()).collect();
            lichen_chain::Chain::esploras(&urls, a.network)
        }
    }
}

fn main() -> Result<()> {
    let a = Args::parse();
    if let Cmd::Init = a.cmd {
        let mut l2 = <LiquidWallet as lichen_wallet::WalletL2>::create(&lexit_wallet::hub_info(&a.hub)?)?;
        if let Some(e) = &a.esplora_liquid {
            l2.esplora = e.clone();
        }
        let w = Wallet::create_with(&a.wallet, &a.hub, l2)?;
        println!("a wallet for the hub at {}: {}", a.hub, w.path().display());
        return Ok(());
    }
    let mut w = Wallet::open(&a.wallet, &a.hub)?;
    if let Some(e) = &a.esplora_liquid {
        w.l2.esplora = e.clone();
    }
    w.progress = Some(std::sync::Arc::new(|m: String| println!("{m}")));
    match a.cmd {
        Cmd::Init => unreachable!(),
        Cmd::Open { deposit, liquidity, reserve, ref payout, resume } => {
            let chain = chain(&a)?;
            let ask = lexit_wallet::Ask { deposit, liquidity, reserve };
            let q = ask.quote(&lexit_wallet::hub_info(&a.hub)?)?;
            println!("the hub's fee for {liquidity} sats of liquidity: {} sats; you'll be paid {} sats of L-BTC, with a cap of {} sats", q.fee, q.minted, q.cap);
            let h = match resume {
                Some(cid) => w.resume_open(&a.hub, &chain, &statement(&a)?, ask, cid)?,
                None => {
                    let payout = match payout.as_deref() {
                        Some(p) => p.parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()?.require_network(a.network.bitcoin()).context("--payout")?.script_pubkey(),
                        None => {
                            let spk = w.payout().spk();
                            chain.watch(&spk)?;
                            println!("payouts to this wallet's own address: {}", w.payout_address(a.network));
                            spk
                        }
                    };
                    let pay = |addr: &bitcoin::Address, amount: bitcoin::Amount| -> Result<()> {
                        println!("pay {} sats to {addr}\n(waiting for it to confirm)", amount.to_sat());
                        Ok(())
                    };
                    w.open_contract(&a.hub, &chain, &statement(&a)?, ask, payout, &pay)?
                }
            };
            let t = &h.record.terms;
            println!("contract {}: deposit {} sats ({} of L-BTC minted), cap {} sats, T_close {}; funding {}", h.record.cid(), t.deposit, h.record.minted(), t.v_max().to_sat(), t.t_close, h.record.funding.txid);
            println!("its return address on Liquid: {}", lexit_wallet::return_address(&w, h.record.cid())?);
        }
        Cmd::Contracts => {
            for h in &w.contracts {
                let t = &h.record.terms;
                let state = h.closed.map_or("open".to_string(), |x| format!("closed by {x}"));
                println!("contract {}: deposit {} sats, cap {} sats, T_close {}; {state}", h.record.cid(), t.deposit, t.v_max().to_sat(), t.t_close);
                println!("  funding     {}", h.record.funding);
                if let Ok(addr) = lexit_wallet::return_address(&w, h.record.cid()) {
                    println!("  returns to  {addr}");
                }
                for m in w.l2.mints.iter().filter(|m| m.cid == h.record.cid()) {
                    println!("  mint        {}:{} ({} sats of L-BTC, its key held)", m.txid, m.vout, m.value);
                }
                for r in w.l2.returns.iter().filter(|r| r.cid == h.record.cid()) {
                    println!("  returned    {}:{} ({} sats)", r.txid, r.vout, r.value);
                }
            }
        }
        Cmd::ReturnAddress { contract } => println!("{}", lexit_wallet::return_address(&w, contract)?),
        Cmd::RecordReturn { contract, ref txid, vout, value, ref r_a, ref r_v } => {
            w.l2.record_return(Return { cid: contract, txid: txid.clone(), vout, value, r_a: r_a.clone(), r_v: r_v.clone() })?;
            w.save()?;
            println!("recorded: {value} sats returned for contract {contract}");
        }
        Cmd::Withdraw { contract, amount, escalate } => match w.withdraw(&chain(&a)?, &a.hub, &statement(&a)?, contract, amount) {
            Ok(txid) => println!("withdrew from contract {contract}: the fold is {txid}"),
            Err(e) if escalate => {
                eprintln!("The cooperative withdrawal failed: {e:#}\nExiting on Bitcoin");
                let (outcome, swept) = w.exit(&chain(&a)?, &statement(&a)?, contract)?;
                println!("{outcome:?}; swept {swept:?}");
            }
            Err(e) => return Err(e.context(format!("the hub didn't fold: `lexit exit {contract}` claims your returns on Bitcoin"))),
        },
        Cmd::Exit { contract } => {
            let (outcome, swept) = w.exit(&chain(&a)?, &statement(&a)?, contract)?;
            println!("{outcome:?}; swept {swept:?}");
        }
        Cmd::PostDefault { contract } => println!("posted contract {contract}'s default: {}", w.post_default(&chain(&a)?, &statement(&a)?, contract)?),
        Cmd::Refund { contract, fee } => println!("refunded the deposit for contract {contract}: {}", w.refund(&chain(&a)?, contract, bitcoin::Amount::from_sat(fee))?),
        Cmd::SweepFees { contract, fee, force } => println!("swept contract {contract}'s fee budget: {}", w.sweep_fees(&chain(&a)?, contract, bitcoin::Amount::from_sat(fee), force)?),
        Cmd::Payout => {
            let chain = chain(&a)?;
            let p = w.payout();
            chain.watch(&p.spk())?;
            println!("{}\n{} sats", w.payout_address(a.network), p.balance(&chain)?.to_sat());
        }
        Cmd::SendPayout { ref to, amount } => {
            let to = to.parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()?.require_network(a.network.bitcoin()).context("the address")?;
            println!("paid: {}", w.send_payout(&chain(&a)?, &to.script_pubkey(), amount.map(bitcoin::Amount::from_sat))?);
        }
    }
    Ok(())
}
