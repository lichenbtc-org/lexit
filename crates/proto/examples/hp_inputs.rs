//! The hub's statement's inputs, one per ground, for measuring the guest
//! (`lexit-r0 exec-hp <input> [<guest>]`): each checked natively here, then
//! written to `<dir>/<ground>.bin`.
//!
//! `request` is synthetic: a contract's funding spending its deposit coin,
//! `request` spending the funding with a claim posted, in a block of
//! regtest difficulty with ten more on it (the contract's work). Liquid is
//! mainnet's block 4,153,000 (its federation's 11-of-15 signatures, its
//! two transactions) where a ground can use it; the worst case of "not a
//! return" (an output paying the contract's return script whose
//! commitments are opened, the value wrong) needs a return, so its block
//! is ours: Liquid-shaped (mainnet's proposed params, about 3 kB), signed
//! 11-of-15 by test keys.
//!
//!     cargo run -p lexit-proto --example hp_inputs -- <dir>

use anyhow::{ensure, Context, Result};
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::{absolute, transaction, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use elements::secp256k1_zkp::{Generator, PedersenCommitment, Tweak};
use lexit_elements::{commit, header, merkle, sha256, sha256d, tx, LBTC};
use lexit_statement::btc::work;
use lexit_statement::{check, posted_items, Block, Citation, Claim, Ground, HubInput, Request, Terms, POSTED_ITEMS};
use sha2::{Digest, Sha256};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../elements/tests/data");
/// Liquid's block signing script (P2WSH of the 11-of-15).
const LIQUID_SCRIPT: &str = "00207f1a37f651f3cdae15be58ecca344c4c424d3ac250e00452bdb16a9777e6be74";
const C: u32 = 1000;
/// Regtest's bits: about 2 work a block.
const BITS: u32 = 0x207f_ffff;

fn data(name: &str) -> Result<Vec<u8>> {
    Ok(hex::decode(std::fs::read_to_string(format!("{DATA}/{name}"))?.trim())?)
}

fn key(n: u8) -> SecretKey {
    SecretKey::from_slice(&Sha256::digest([b'k', n]).into_iter().collect::<Vec<u8>>()).unwrap()
}

/// A header of `bits` on `prev`, mined.
fn mine(prev: [u8; 32], merkle_root: [u8; 32], time: u32) -> [u8; 80] {
    let mut h = [0u8; 80];
    h[..4].copy_from_slice(&0x2000_0000u32.to_le_bytes());
    h[4..36].copy_from_slice(&prev);
    h[36..68].copy_from_slice(&merkle_root);
    h[68..72].copy_from_slice(&time.to_le_bytes());
    h[72..76].copy_from_slice(&BITS.to_le_bytes());
    for nonce in 0u32.. {
        h[76..].copy_from_slice(&nonce.to_le_bytes());
        if lexit_statement::btc::Header(h).pow_ok() {
            return h;
        }
    }
    unreachable!()
}

/// `request`'s witness: the claim posted, her signatures' digits on `E`
/// and on `b` (only the digits are read: Bitcoin checked the signatures),
/// the 2-of-2's signatures, the script and the control block.
fn request_witness(posted: &[u8], e: &[u8; 32], b: u32) -> Witness {
    let mut items: Vec<Vec<u8>> = posted_items(posted).unwrap().into_iter().map(<[u8]>::to_vec).collect();
    assert_eq!(items.len(), POSTED_ITEMS);
    let digits = |bytes: &[u8], checksum: usize| -> Vec<Vec<u8>> {
        let mut out = vec![];
        for d in bytes.iter().flat_map(|x| [x >> 4, x & 15]).chain(std::iter::repeat_n(0, checksum)) {
            out.push(vec![0xaa; 20]);
            out.push(if d == 0 { vec![] } else { vec![d] });
        }
        out
    };
    items.extend(digits(e, 3));
    items.extend(digits(&b.to_be_bytes(), 2));
    items.extend([vec![1; 64], vec![2; 64], vec![0x51; 100], vec![0xc0; 33]]);
    Witness::from_slice(&items)
}

/// `request` in its block (time `time`), the contract's work on it: the
/// statement's `Request`, the headers on it, and the work they make.
fn bitcoin_side(deposit: OutPoint, posted: &[u8], e: &[u8; 32], b: u32, time: u32) -> (Request, Vec<[u8; 80]>, u128) {
    let funding = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn { previous_output: deposit, script_sig: ScriptBuf::new(), sequence: Sequence::MAX, witness: Witness::new() }],
        output: vec![TxOut { value: Amount::from_sat(1_000_000), script_pubkey: ScriptBuf::from_bytes([&[0x51, 0x20][..], &[7; 32]].concat()) }],
    };
    let request = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn { previous_output: OutPoint { txid: funding.compute_txid(), vout: 0 }, script_sig: ScriptBuf::new(), sequence: Sequence::MAX, witness: request_witness(posted, e, b) }],
        output: vec![TxOut { value: Amount::from_sat(990_000), script_pubkey: ScriptBuf::from_bytes([&[0x51, 0x20][..], &[8; 32]].concat()) }],
    };
    // the coinbase commits the witness tree: [0, wtxid(request)]
    let reserved = [0u8; 32];
    let wroot = merkle::root(&[[0; 32], request.compute_wtxid().to_byte_array()]).unwrap();
    let commitment = sha256d(&[wroot, reserved].concat());
    let coinbase = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn { previous_output: OutPoint::null(), script_sig: ScriptBuf::from_bytes(vec![1, 1]), sequence: Sequence::MAX, witness: Witness::from_slice(&[reserved]) }],
        output: vec![TxOut { value: Amount::ZERO, script_pubkey: ScriptBuf::from_bytes([&[0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed][..], &commitment].concat()) }],
    };
    let bare = |t: &Transaction| {
        let mut t = t.clone();
        t.input.iter_mut().for_each(|i| i.witness = Witness::new());
        serialize(&t)
    };
    let ids = [coinbase.compute_txid().to_byte_array(), request.compute_txid().to_byte_array()];
    let block = mine([0; 32], merkle::root(&ids).unwrap(), time);
    let mut headers = vec![];
    let mut prev = lexit_statement::btc::Header(block).hash();
    for i in 0..10 {
        let h = mine(prev, [i; 32], time + 1 + u32::from(i));
        prev = lexit_statement::btc::Header(h).hash();
        headers.push(h);
    }
    let req = Request { funding: bare(&funding), tx: serialize(&request), header: block, index: 1, path: vec![[0; 32]], coinbase: bare(&coinbase), coinbase_path: vec![ids[1]], reserved };
    (req, headers, 10 * work(BITS))
}

/// Our own Liquid-shaped block at `height`: mainnet's proposed params,
/// current params naming our 11-of-15, `txs` in it, signed. Its header and
/// the signing script.
fn our_block(height: u32, time: u32, txids: &[[u8; 32]]) -> Result<(Vec<u8>, Vec<u8>)> {
    let secp = Secp256k1::new();
    let keys: Vec<SecretKey> = (0..15).map(key).collect();
    let mut ws = vec![0x5b];
    for k in &keys {
        ws.push(33);
        ws.extend(k.public_key(&secp).serialize());
    }
    ws.extend([0x5f, 0xae]);
    let script = [&[0x00, 0x20][..], &sha256(&ws)].concat();
    let mainnet = data("block-4153000.header.hex")?;
    let m = header::parse(&mainnet).context("mainnet's header")?;
    // mainnet's proposed params' bytes: after the current params
    let header::Params::Compact { elided_root, .. } = m.current else { anyhow::bail!("mainnet's current params aren't compact") };
    let current_len = 1 + 1 + LIQUID_SCRIPT.len() / 2 + 4 + 32;
    // the fixed part: version, prev, root, time, height (76 bytes)
    let proposed = &m.unsigned[76 + current_len..];
    let mut u = vec![];
    u.extend(0xa000_0000u32.to_le_bytes());
    u.extend([0x11; 32]);
    u.extend(merkle::root(txids).unwrap());
    u.extend(time.to_le_bytes());
    u.extend(height.to_le_bytes());
    u.push(1);
    u.push(34);
    u.extend(&script);
    u.extend(1325u32.to_le_bytes());
    u.extend(elided_root);
    u.extend(proposed);
    let hash = sha256d(&u);
    let mut witness: Vec<Vec<u8>> = vec![vec![]];
    for k in &keys[..11] {
        let mut sig = secp.sign_ecdsa(&Message::from_digest(hash), k).serialize_der().to_vec();
        sig.push(1);
        witness.push(sig);
    }
    witness.push(ws);
    u.push(witness.len() as u8);
    for w in &witness {
        if w.len() < 0xfd {
            u.push(w.len() as u8);
        } else {
            u.push(0xfd);
            u.extend((w.len() as u16).to_le_bytes());
        }
        u.extend(w);
    }
    Ok((u, script))
}

/// A transaction paying `v` sats of L-BTC, blinded by `r_a` and `r_v`, to
/// `script`, and an explicit fee: no witness (the statement reads none).
fn return_tx(script: &[u8], v: u64, r_a: &[u8; 32], r_v: &[u8; 32]) -> Vec<u8> {
    let secp = elements::secp256k1_zkp::Secp256k1::new();
    let a = Generator::new_blinded(&secp, elements::AssetId::LIQUID_BTC.into_tag(), Tweak::from_slice(r_a).unwrap());
    let c = PedersenCommitment::new(&secp, v, Tweak::from_slice(r_v).unwrap(), a);
    let mut t = vec![];
    t.extend(2u32.to_le_bytes());
    t.push(0);
    t.push(1);
    t.extend([0x22; 32]);
    t.extend(0u32.to_le_bytes());
    t.push(0);
    t.extend(u32::MAX.to_le_bytes());
    t.push(2);
    t.extend(a.serialize());
    t.extend(c.serialize());
    t.push(2);
    t.extend([3; 32]); // a nonce: an ECDH key
    t.push(script.len() as u8);
    t.extend(script);
    t.push(1);
    t.extend(LBTC);
    t.push(1);
    t.extend(100u64.to_be_bytes());
    t.push(0);
    t.push(0);
    t.extend(0u32.to_le_bytes());
    assert!(tx::parse(&t).is_some());
    t
}

/// A ground's case: its name, the terms' signing script, the claim,
/// `request`'s time, the ground, and whether `E` is wrong.
type Case = (&'static str, Vec<u8>, Claim, u32, Ground, bool);

fn main() -> Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).context("usage: hp_inputs <dir>")?);
    std::fs::create_dir_all(&dir)?;
    let secp = Secp256k1::new();
    let hub = key(99).x_only_public_key(&secp).0.serialize();
    let deposit = OutPoint { txid: bitcoin::Txid::from_byte_array([0xd0; 32]), vout: 0 };
    let deposit_s = lexit_statement::OutPoint { txid: [0xd0; 32], vout: 0 };
    let liquid_script = hex::decode(LIQUID_SCRIPT)?;
    let m_header = data("block-4153000.header.hex")?;
    let m_time = header::parse(&m_header).unwrap().time;
    let (cb, t1) = (data("tx-coinbase-4153000.hex")?, data("tx-1-4153000.hex")?);
    let m_txids = vec![tx::parse(&cb).unwrap().txid, tx::parse(&t1).unwrap().txid];
    let (r_a, r_v) = ([0x31; 32], [0x32; 32]);

    // each ground: its terms' script, the claim, `request`'s time, the
    // ground; E right unless `bad_e`
    let mut cases: Vec<Case> = vec![];
    let cite = |height, index, value| Citation { height, index, vout: 0, value, r_a, r_v };
    let one = |c: Citation| Claim { cited: vec![c] };
    cases.push(("malformed", liquid_script.clone(), one(cite(4_153_000, 1, 50_000)), m_time, Ground::Malformed, true));
    cases.push((
        "not-a-return-script",
        liquid_script.clone(),
        one(cite(4_153_000, 1, 50_000)),
        m_time,
        Ground::NotAReturn { which: 0, block: Block { header: m_header.clone(), txids: m_txids.clone(), tx: t1.clone() } },
        false,
    ));
    cases.push((
        "not-a-return-no-tx",
        liquid_script.clone(),
        one(cite(4_153_000, 5, 50_000)),
        m_time,
        Ground::NotAReturn { which: 0, block: Block { header: m_header.clone(), txids: m_txids.clone(), tx: vec![] } },
        false,
    ));
    cases.push(("too-high", liquid_script.clone(), one(cite(4_153_010, 1, 50_000)), m_time - 3 * 3600 - 100, Ground::TooHigh { which: 0, header: m_header.clone() }, false));
    // the worst case: a return, opened, its value not the one cited
    {
        let t = Terms { signblockscript: vec![], asset: LBTC, generator: commit::LBTC_GENERATOR, hub, from: 4_000_000, drift: 3 * 3600, work: 10 * work(BITS), unit: 1 };
        // the script (whatever the block holds), for D, for the return script
        let (_, script0) = our_block(4_200_000, m_time, &[[0; 32]])?;
        let d = Terms { signblockscript: script0.clone(), ..t }.d();
        let ret = lexit_statement::return_script(&hub, &d, C).context("a return script")?;
        let rtx = return_tx(&ret, 50_000, &r_a, &r_v);
        let txids = vec![[0x44; 32], tx::parse(&rtx).unwrap().txid];
        let (h, script) = our_block(4_200_000, m_time, &txids)?;
        ensure!(script == script0);
        cases.push(("not-a-return-opening", script.clone(), one(cite(4_200_000, 1, 50_001)), m_time, Ground::NotAReturn { which: 0, block: Block { header: h.clone(), txids: txids.clone(), tx: rtx.clone() } }, false));
        // calibration: the same block, its fee output cited (another
        // script: nothing opened)
        cases.push(("not-a-return-our-script", script, one(Citation { vout: 1, ..cite(4_200_000, 1, 50_000) }), m_time, Ground::NotAReturn { which: 0, block: Block { header: h, txids, tx: rtx } }, false));
    }

    for (name, script, claim, time, ground, bad_e) in cases {
        let terms = Terms { signblockscript: script, asset: LBTC, generator: commit::LBTC_GENERATOR, hub, from: 4_000_000, drift: 3 * 3600, work: 10 * work(BITS), unit: 1 };
        let b = (claim.total() / terms.unit) as u32;
        let e = if bad_e { [0; 32] } else { claim.fingerprint(C, &deposit_s, b) };
        let posted = claim.to_bytes();
        let (request, headers, w) = bitcoin_side(deposit, &posted, &e, b, time);
        ensure!(w == terms.work);
        let input = HubInput { c: C, deposit: deposit_s, terms, request, headers, ground };
        let journal = check(&input).map_err(|r| anyhow::anyhow!("{name}: the statement rejects it: {r:?}"))?;
        let bytes = input.to_bytes();
        ensure!(HubInput::from_bytes(&bytes).as_ref() == Ok(&input), "{name}: the encoding round-trips");
        std::fs::write(dir.join(format!("{name}.bin")), &bytes)?;
        println!("{name}: {} bytes, journal {}", bytes.len(), hex::encode(journal));
    }
    Ok(())
}
