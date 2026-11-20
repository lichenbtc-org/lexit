//! `lexit-r0`: proves Lexit's guest, the hub's statement.
//!
//! - `hp <hub input> <out> [<packed guest>]`: prove the hub's statement
//!   (`lexit_statement::check`), on its input
//!   (its bytes, `Input::to_bytes`); writes the succinct receipt;
//!   `exec-hp <hub input> [<packed guest>]` runs the guest without proving
//!   (its cycles), the committed one or another build;
//! - `wrap <receipt> <out>`: the Groth16 receipt of a succinct receipt of
//!   the hub's statement (x86 Linux with Docker: RISC Zero's gnark step);
//! - `hp-proof <hub input> <out>`: `hp` then `wrap`, writing the Groth16
//!   seal (256 bytes: A, B, C uncompressed, big-endian, RISC Zero's), what
//!   the hub asserts (`lichen_babe::risc0`);
//! - `pack <guest ELF> <out>`: a guest's ELF with RISC Zero's kernel (the
//!   program binary the image id is of), for r0/build-guest.sh;
//! - `image-id`: the committed guest's image id; `check-id`: that it is
//!   its committed `.id`; `pack-id <bin>`: a packed binary's.
//!
//! The guest proved is the committed `r0/guests/hp.bin`.

use std::io::{Read, Write};

use anyhow::{bail, ensure, Context, Result};
use risc0_zkvm::{compute_image_id, default_executor, default_prover, Digest, ExecutorEnv, InnerReceipt, ProverOpts, Receipt, ReceiptClaim};

const GUESTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../guests");

/// A committed guest (`hp`).
fn committed(name: &str) -> Result<Vec<u8>> {
    let p = format!("{GUESTS}/{name}.bin");
    std::fs::read(&p).with_context(|| format!("the committed guest, {p}"))
}

fn image_id(guest: &[u8]) -> Result<Digest> {
    compute_image_id(guest).context("the guest's image id")
}

fn read(path: &str) -> Result<Vec<u8>> {
    if path == "-" {
        let mut b = vec![];
        std::io::stdin().read_to_end(&mut b)?;
        return Ok(b);
    }
    std::fs::read(path).with_context(|| format!("reading {path}"))
}

fn write(path: &str, b: &[u8]) -> Result<()> {
    if path == "-" {
        return Ok(std::io::stdout().write_all(b)?);
    }
    std::fs::write(path, b).with_context(|| format!("writing {path}"))
}

fn load(p: &str) -> Result<Receipt> {
    bincode::deserialize(&read(p)?).with_context(|| format!("a receipt, {p}"))
}

/// The hub's statement's input, checked natively: its bytes and journal.
struct Hp {
    bytes: Vec<u8>,
    journal: Vec<u8>,
}

fn hp_input(input: &str) -> Result<Hp> {
    let bytes = read(input)?;
    let hub = lexit_statement::HubInput::from_bytes(&bytes).map_err(|r| anyhow::anyhow!("the hub's input: {r:?}"))?;
    let journal = lexit_statement::check(&hub).map_err(|r| anyhow::anyhow!("the hub's statement rejects the input: {r:?}"))?;
    eprintln!("lexit-r0: contract {}: the claim is false ({})", hub.c, match hub.ground { lexit_statement::Ground::Malformed => "malformed", lexit_statement::Ground::NotAReturn { .. } => "not a return", lexit_statement::Ground::TooHigh { .. } => "too high" });
    Ok(Hp { bytes, journal: journal.to_vec() })
}

/// Bytes as the guest reads them: little-endian words, zero-padded
/// (vlichen's).
fn words(bytes: &[u8]) -> Vec<u32> {
    let mut padded = bytes.to_vec();
    padded.resize(bytes.len().div_ceil(4) * 4, 0);
    padded.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Prove the hub's statement (or, `execute_only`, run it): the succinct
/// receipt, checked.
fn prove_hp(hp: Hp, execute_only: bool) -> Result<Option<Receipt>> {
    prove_hp_with(hp, execute_only, committed("hp")?)
}

fn prove_hp_with(hp: Hp, execute_only: bool, guest: Vec<u8>) -> Result<Option<Receipt>> {
    let id = image_id(&guest)?;
    let mut env = ExecutorEnv::builder();
    env.write(&(hp.bytes.len() as u32))?.write_slice(&words(&hp.bytes));
    let env = env.build()?;
    if execute_only {
        let s = default_executor().execute(env, &guest)?;
        ensure!(s.journal.bytes == hp.journal, "the execution's journal isn't the statement's");
        let user: u64 = s.segments.iter().map(|g| g.cycles as u64).sum();
        eprintln!("lexit-r0: executed hp ({} user cycles, {} segments)", user, s.segments.len());
        return Ok(None);
    }
    let claim = ReceiptClaim::ok(id, hp.journal.clone());
    eprintln!("lexit-r0: the claim digest, the Groth16 public input's: {}", risc0_zkvm::sha::Digestible::digest(&claim));
    let t = std::time::Instant::now();
    let info = default_prover().prove_with_opts(env, &guest, &ProverOpts::succinct())?;
    eprintln!("lexit-r0: proved hp in {:.1}s ({} user cycles, {} segments)", t.elapsed().as_secs_f64(), info.stats.user_cycles, info.stats.segments);
    info.receipt.verify(id)?;
    ensure!(info.receipt.journal.bytes == hp.journal, "the receipt's journal isn't the statement's");
    Ok(Some(info.receipt))
}

/// The Groth16 receipt of a succinct receipt of the hub's statement,
/// checked.
fn wrap(r: &Receipt) -> Result<Receipt> {
    let id = image_id(&committed("hp")?)?;
    r.verify(id).context("a receipt of the hub's statement")?;
    let t = std::time::Instant::now();
    let g = default_prover().compress(&ProverOpts::groth16(), r)?;
    eprintln!("lexit-r0: wrapped in Groth16 in {:.1}s", t.elapsed().as_secs_f64());
    g.verify(id)?;
    Ok(g)
}

/// A Groth16 receipt's seal (256 bytes).
fn seal(r: &Receipt) -> Result<Vec<u8>> {
    let InnerReceipt::Groth16(g) = &r.inner else { bail!("not a Groth16 receipt") };
    ensure!(g.seal.len() == 256, "a Groth16 seal is 256 bytes, not {}", g.seal.len());
    Ok(g.seal.clone())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["pack", elf, out] => {
            let bin = risc0_binfmt::ProgramBinary::new(&read(elf)?, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
            write(out, &bin)?;
            println!("{}", image_id(&bin)?);
            Ok(())
        }
        ["hp", input, out] => {
            let r = prove_hp(hp_input(input)?, false)?.context("a receipt")?;
            write(out, &bincode::serialize(&r)?)
        }
        ["hp", input, out, guest] => {
            let r = prove_hp_with(hp_input(input)?, false, read(guest)?)?.context("a receipt")?;
            write(out, &bincode::serialize(&r)?)
        }
        ["exec-hp", input] => prove_hp(hp_input(input)?, true).map(|_| ()),
        // another build of the guest (packed): a native, unreproducible
        // build, for its cycles
        ["exec-hp", input, guest] => prove_hp_with(hp_input(input)?, true, read(guest)?).map(|_| ()),
        ["wrap", receipt, out] => write(out, &bincode::serialize(&wrap(&load(receipt)?)?)?),
        ["hp-proof", input, out] => {
            let r = prove_hp(hp_input(input)?, false)?.context("a receipt")?;
            write(out, &seal(&wrap(&r)?)?)?;
            eprintln!("lexit-r0: {out}: the Groth16 seal of the hub's statement");
            Ok(())
        }
        ["image-id"] => {
            println!("{}", image_id(&committed("hp")?)?);
            Ok(())
        }
        ["pack-id", bin] => {
            println!("{}", image_id(&read(bin)?)?);
            Ok(())
        }
        ["check-id"] => {
            for name in ["hp"] {
                // a guest not built yet has neither file
                if !std::path::Path::new(&format!("{GUESTS}/{name}.bin")).exists() && !std::path::Path::new(&format!("{GUESTS}/{name}.id")).exists() {
                    continue;
                }
                let id = image_id(&committed(name)?)?.to_string();
                let c = std::fs::read_to_string(format!("{GUESTS}/{name}.id"))?;
                ensure!(c.trim() == id, "{name}.id is {}, the committed {name}.bin's image id {id}", c.trim());
            }
            Ok(())
        }
        _ => bail!("usage: lexit-r0 hp <hub input> <out> [<packed guest>] | exec-hp <hub input> [<packed guest>] | wrap <receipt> <out> | hp-proof <hub input> <out> | pack <guest ELF> <out> | pack-id <bin> | image-id | check-id"),
    }
}
