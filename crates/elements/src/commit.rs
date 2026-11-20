//! A confidential output opened: its asset commitment, a blinded
//! generator `A' = H_asset + r_a·G`, and its value commitment, a Pedersen
//! commitment `V = v·A' + r_v·G` (secp256k1-zkp's).
//!
//! `H_asset` is the asset's generator, secp256k1-zkp's hash of its id to
//! the curve. The statement never computes it: a contract pins L-BTC's
//! ([`LBTC_GENERATOR`]), checked in the tests against secp256k1-zkp.
//!
//! secp256k1-zkp serializes these points as a prefix and the x
//! coordinate, the prefix saying whether y is a square mod p (not whether
//! it is odd, as a compressed public key does): a generator `0a` (a
//! square) or `0b`, a commitment `08` or `09`.

use k256::elliptic_curve::sec1::{EncodedPoint, FromEncodedPoint, ToEncodedPoint};
use k256::elliptic_curve::PrimeField;
use k256::{AffinePoint, FieldBytes, FieldElement, ProjectivePoint, Scalar, Secp256k1};

/// L-BTC's generator, as secp256k1-zkp serializes it (`0a`/`0b` ‖ x).
pub const LBTC_GENERATOR: [u8; 33] = [
    0x0a, 0x0a, 0x48, 0x8d, 0xe4, 0x89, 0x9d, 0x0a, 0xe7, 0x57, 0xf6, 0xcf, 0x83, 0x68, 0x66, 0x31, 0x84, 0xd1, 0x64, 0x10, 0x61, 0x11, 0xed, 0x9e, 0xae, 0xcf, 0x51, 0x0e, 0x35, 0x28, 0x2d, 0xdc, 0x6d,
];

const GENERATOR: u8 = 0x0a;
const COMMITMENT: u8 = 0x08;

fn is_square(y: &FieldElement) -> bool {
    bool::from(y.sqrt().is_some())
}

/// A point serialized secp256k1-zkp's way, the prefix `base` (y a square)
/// or `base + 1`.
fn decode(b: &[u8; 33], base: u8) -> Option<ProjectivePoint> {
    let square = match b[0] {
        p if p == base => true,
        p if p == base + 1 => false,
        _ => return None,
    };
    let x = Option::<FieldElement>::from(FieldElement::from_bytes(&FieldBytes::from(<[u8; 32]>::try_from(&b[1..]).ok()?)))?;
    let y2 = (x.square() * x + FieldElement::from(7u64)).normalize();
    let mut y = Option::<FieldElement>::from(y2.sqrt())?.normalize();
    if is_square(&y) != square {
        y = y.negate(1).normalize();
    }
    let ep = EncodedPoint::<Secp256k1>::from_affine_coordinates(&x.to_bytes(), &y.to_bytes(), false);
    Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&ep)).map(ProjectivePoint::from)
}

fn encode(p: &ProjectivePoint, base: u8) -> Option<[u8; 33]> {
    let ep = p.to_affine().to_encoded_point(false);
    let (x, y) = (ep.x()?, ep.y()?);
    let y = Option::<FieldElement>::from(FieldElement::from_bytes(y))?;
    let mut out = [0u8; 33];
    out[0] = if is_square(&y) { base } else { base + 1 };
    out[1..].copy_from_slice(x);
    Some(out)
}

/// A 32-byte blinding factor as a scalar (big-endian, below the order).
fn scalar(b: &[u8; 32]) -> Option<Scalar> {
    Option::from(Scalar::from_repr((*b).into()))
}

/// The asset commitment of `generator` (an asset's, serialized) blinded by
/// `r_a`.
pub fn blinded_generator(generator: &[u8; 33], r_a: &[u8; 32]) -> Option<[u8; 33]> {
    let h = decode(generator, GENERATOR)?;
    encode(&(h + ProjectivePoint::GENERATOR * scalar(r_a)?), GENERATOR)
}

/// The value commitment of `v` under the blinded generator `asset`
/// (serialized), blinded by `r_v`.
pub fn value_commitment(asset: &[u8; 33], v: u64, r_v: &[u8; 32]) -> Option<[u8; 33]> {
    let a = decode(asset, GENERATOR)?;
    encode(&(a * Scalar::from(v) + ProjectivePoint::GENERATOR * scalar(r_v)?), COMMITMENT)
}

/// Whether a confidential output's commitments open to `v` of the asset of
/// `generator`, with blinding factors `r_a` and `r_v`.
pub fn opens(asset: &[u8; 33], value: &[u8; 33], generator: &[u8; 33], v: u64, r_a: &[u8; 32], r_v: &[u8; 32]) -> bool {
    blinded_generator(generator, r_a).is_some_and(|a| a == *asset) && value_commitment(asset, v, r_v).is_some_and(|c| c == *value)
}
