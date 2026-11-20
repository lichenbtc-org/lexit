//! The hub's statement guest (`liquid_lichen_statement::check`): "the exit
//! claim `request` posts is false". It reads `request` in its Bitcoin
//! block with the contract's work on it, the claim posted there, and the
//! ground: a Liquid block signed by the federation that shows a cited
//! return isn't one, or that a cited height wasn't reached in time. Its
//! input, read raw: the statement's input in bytes (`HubInput::to_bytes`).
//! Its journal: `c ‖ D ‖ H(O_D)`, fixed at the contract's opening.

use liquid_lichen_statement::{check, HubInput};
use risc0_zkvm::guest::env;

fn main() {
    let n: u32 = env::read();
    let mut words = vec![0u32; (n as usize).div_ceil(4)];
    env::read_slice(&mut words);
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).take(n as usize).collect();
    let input = HubInput::from_bytes(&bytes).expect("the statement's input");
    let journal = check(&input).expect("the hub's statement");
    env::commit_slice(&journal);
}
