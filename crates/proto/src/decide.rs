//! A claim decided against Liquid (the hub's, before it proves; the
//! user's, before she posts): the ground it is false on, or that it holds.

use anyhow::Result;
use liquid_lichen_statement::{not_a_return, Block, Claim, Ground, OutPoint, Terms};

use crate::liquid::{block_at, LiquidView};

/// What a claim is, at Liquid's tip.
pub enum Found {
    /// False: the ground, and why in words.
    False(Ground, String),
    Holds,
    /// Not yet decidable: a cited block is still to come, within the drift.
    Wait(String),
}

/// Decide the claim `posted` that `request` makes (`b`, `E`; its block's
/// time `req_time`) on contract `c` (terms `t`, deposit coin `deposit`).
#[allow(clippy::too_many_arguments)]
pub fn decide(view: &dyn LiquidView, t: &Terms, c: u32, deposit: &OutPoint, b: u32, e: &[u8; 32], posted: &[u8], req_time: u32) -> Result<Found> {
    let claim = match Claim::from_bytes(posted) {
        Ok(cl) if cl.well_formed(t.from) && cl.fingerprint(c, deposit, b) == *e && cl.total() / t.unit >= u64::from(b) => cl,
        _ => return Ok(Found::False(Ground::Malformed, "the claim is malformed".into())),
    };
    let d = t.d();
    let tip = view.height()?;
    for (w, cite) in claim.cited.iter().enumerate() {
        if cite.height > tip {
            let header = view.header(&view.block_hash(tip)?)?;
            let late = liquid_lichen_elements::header::parse(&header).is_some_and(|h| u64::from(h.time) > u64::from(req_time) + u64::from(t.drift));
            if late {
                return Ok(Found::False(Ground::TooHigh { which: w as u8, header }, format!("it cites block {}, beyond Liquid's tip ({tip}) at its time", cite.height)));
            }
            return Ok(Found::Wait(format!("it cites block {}, not yet on Liquid (tip {tip})", cite.height)));
        }
        let (header, txids) = block_at(view, cite.height)?;
        let tx = match txids.get(cite.index as usize) {
            Some(id) => view.tx(id)?,
            None => vec![],
        };
        let block = Block { header, txids, tx };
        if not_a_return(t, &d, c, cite, &block) {
            return Ok(Found::False(Ground::NotAReturn { which: w as u8, block }, format!("block {} shows no return at {}:{}", cite.height, cite.index, cite.vout)));
        }
    }
    Ok(Found::Holds)
}
