//! Liquid as both sides see it: a view of the chain ([`LiquidView`]: the
//! hub's elementsd over RPC, [`Rpc`]; the user's Esplora, [`Esplora`]),
//! the network ([`LiquidNet`]), and the keys and addresses a contract
//! uses on Liquid: the mint's ([`mint_key`]) and the returns'
//! ([`return_address`]).

use anyhow::{bail, ensure, Context, Result};
use bitcoin::secp256k1::{PublicKey, Secp256k1, XOnlyPublicKey};
use elements::AddressParams;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use lichen_proto::Hash;

/// A Liquid network: what a hub's deployment names, and both sides check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiquidNet {
    /// Liquid itself (`liquidv1`).
    Liquid,
    /// Blockstream's Liquid testnet (`liquidtestnet`).
    Testnet,
    /// A local Elements chain (`elementsregtest`), its block signing
    /// script our own.
    Regtest,
}

impl LiquidNet {
    pub fn address_params(self) -> &'static AddressParams {
        match self {
            LiquidNet::Liquid => &AddressParams::LIQUID,
            LiquidNet::Testnet => &AddressParams::LIQUID_TESTNET,
            LiquidNet::Regtest => &AddressParams::ELEMENTS,
        }
    }
    /// Its policy asset (L-BTC), if fixed: a regtest chain's is its own.
    pub fn policy_asset(self) -> Option<Hash> {
        match self {
            LiquidNet::Liquid => Some(lexit_elements::LBTC),
            LiquidNet::Testnet => internal("144c654344aa716d6f3abcc1ca90e5641e4e2a7f633bc09fe3baf64585819a49").ok(),
            LiquidNet::Regtest => None,
        }
    }
    /// Its public Esplora, if any.
    pub fn default_esplora(self) -> Option<&'static str> {
        match self {
            LiquidNet::Liquid => Some("https://blockstream.info/liquid/api"),
            LiquidNet::Testnet => Some("https://blockstream.info/liquidtestnet/api"),
            LiquidNet::Regtest => None,
        }
    }
    /// The Liquid blocks after which the hub may take back a mint (its
    /// refund leaf): long past lichen's deposit timeout, by which the
    /// funding is out or the deposit refunded, so that a mint can't be
    /// taken back from a user still owed it. Three days of Liquid's
    /// minute blocks; a regtest chain's blocks come when the tests make
    /// them.
    pub fn mint_timeout(self) -> u16 {
        match self {
            LiquidNet::Liquid | LiquidNet::Testnet => 4_320,
            LiquidNet::Regtest => 1_000,
        }
    }
    /// The seconds the statement allows between a Liquid block's time and
    /// `request`'s block time: Bitcoin's block times run up to two hours
    /// ahead and about an hour behind; regtest's are the machine's.
    pub fn drift(self) -> u32 {
        match self {
            LiquidNet::Regtest => 600,
            _ => 3 * 3600,
        }
    }
}

/// What both sides read of Liquid.
pub trait LiquidView {
    /// The tip's height.
    fn height(&self) -> Result<u32>;
    /// The block hash at `height` (displayed order, hex).
    fn block_hash(&self, height: u32) -> Result<String>;
    /// A block's header, serialized with its witness.
    fn header(&self, hash: &str) -> Result<Vec<u8>>;
    /// A block's txids, in order (internal byte order).
    fn txids(&self, hash: &str) -> Result<Vec<Hash>>;
    /// A transaction, serialized.
    fn tx(&self, txid: &Hash) -> Result<Vec<u8>>;
    /// The block a transaction is in (height, hash), if confirmed.
    fn tx_block(&self, txid: &Hash) -> Result<Option<(u32, String)>>;
}

fn display(h: &Hash) -> String {
    let mut b = *h;
    b.reverse();
    hex::encode(b)
}

fn internal(s: &str) -> Result<Hash> {
    let mut h: Hash = hex::decode(s)?.try_into().map_err(|_| anyhow::anyhow!("a hash of 32 bytes"))?;
    h.reverse();
    Ok(h)
}

/// elementsd over JSON-RPC, by its cookie file (the hub's node; a test's).
#[derive(Clone, Debug)]
pub struct Rpc {
    pub url: String,
    pub cookie: std::path::PathBuf,
    /// The node's wallet that pays the mints and holds the returns.
    pub wallet: String,
}

impl Rpc {
    pub fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.call_at(&self.url, method, params)
    }
    /// A call to the node's wallet.
    pub fn wallet_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.call_at(&format!("{}/wallet/{}", self.url.trim_end_matches('/'), self.wallet), method, params)
    }
    fn call_at(&self, url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let auth = std::fs::read_to_string(&self.cookie).with_context(|| format!("elementsd's cookie {}", self.cookie.display()))?;
        let body = serde_json::json!({ "jsonrpc": "1.0", "id": "lexit", "method": method, "params": params });
        let r = ureq::post(url).set("Authorization", &format!("Basic {}", base64(auth.trim().as_bytes()))).send_json(body);
        let v: serde_json::Value = match r {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(_, r)) => r.into_json()?,
            Err(e) => return Err(e).with_context(|| format!("elementsd at {url}")),
        };
        if !v["error"].is_null() {
            bail!("elementsd {method}: {}", v["error"]);
        }
        Ok(v["result"].clone())
    }
}

/// Standard base64, for the RPC's basic authentication.
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

impl LiquidView for Rpc {
    fn height(&self) -> Result<u32> {
        Ok(self.call("getblockcount", serde_json::json!([]))?.as_u64().context("a height")? as u32)
    }
    fn block_hash(&self, height: u32) -> Result<String> {
        Ok(self.call("getblockhash", serde_json::json!([height]))?.as_str().context("a block hash")?.to_string())
    }
    fn header(&self, hash: &str) -> Result<Vec<u8>> {
        Ok(hex::decode(self.call("getblockheader", serde_json::json!([hash, false]))?.as_str().context("a header's hex")?)?)
    }
    fn txids(&self, hash: &str) -> Result<Vec<Hash>> {
        let b = self.call("getblock", serde_json::json!([hash, 1]))?;
        b["tx"].as_array().context("a block's txids")?.iter().map(|t| internal(t.as_str().context("a txid")?)).collect()
    }
    fn tx(&self, txid: &Hash) -> Result<Vec<u8>> {
        Ok(hex::decode(self.call("getrawtransaction", serde_json::json!([display(txid)]))?.as_str().context("a transaction's hex")?)?)
    }
    fn tx_block(&self, txid: &Hash) -> Result<Option<(u32, String)>> {
        // the node's index (-txindex); a transaction it doesn't know is
        // unconfirmed as far as we can tell
        let Ok(t) = self.call("getrawtransaction", serde_json::json!([display(txid), true])) else { return Ok(None) };
        let Some(hash) = t["blockhash"].as_str() else { return Ok(None) };
        let h = self.call("getblockheader", serde_json::json!([hash, true]))?;
        Ok(Some((h["height"].as_u64().context("a height")? as u32, hash.to_string())))
    }
}

/// A Liquid Esplora (the user's view: no node).
pub struct Esplora {
    pub url: String,
}

impl Esplora {
    fn get(&self, path: &str) -> Result<String> {
        let url = format!("{}/{path}", self.url.trim_end_matches('/'));
        Ok(ureq::get(&url).timeout(std::time::Duration::from_secs(30)).call().with_context(|| format!("GET {url}"))?.into_string()?)
    }
    /// Broadcast a transaction (hex): its txid.
    pub fn broadcast(&self, tx: &[u8]) -> Result<String> {
        let url = format!("{}/tx", self.url.trim_end_matches('/'));
        Ok(ureq::post(&url).send_string(&hex::encode(tx)).with_context(|| format!("POST {url}"))?.into_string()?)
    }
}

impl LiquidView for Esplora {
    fn height(&self) -> Result<u32> {
        Ok(self.get("blocks/tip/height")?.trim().parse()?)
    }
    fn block_hash(&self, height: u32) -> Result<String> {
        Ok(self.get(&format!("block-height/{height}"))?.trim().to_string())
    }
    fn header(&self, hash: &str) -> Result<Vec<u8>> {
        Ok(hex::decode(self.get(&format!("block/{hash}/header"))?.trim())?)
    }
    fn txids(&self, hash: &str) -> Result<Vec<Hash>> {
        let ids: Vec<String> = serde_json::from_str(&self.get(&format!("block/{hash}/txids"))?)?;
        ids.iter().map(|t| internal(t)).collect()
    }
    fn tx(&self, txid: &Hash) -> Result<Vec<u8>> {
        Ok(hex::decode(self.get(&format!("tx/{}/hex", display(txid)))?.trim())?)
    }
    fn tx_block(&self, txid: &Hash) -> Result<Option<(u32, String)>> {
        let s: serde_json::Value = match self.get(&format!("tx/{}/status", display(txid))) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(_) => return Ok(None),
        };
        if s["confirmed"].as_bool() != Some(true) {
            return Ok(None);
        }
        Ok(Some((s["block_height"].as_u64().context("a height")? as u32, s["block_hash"].as_str().context("a block")?.to_string())))
    }
}

/// A block, as a ground shows it, at `height`.
pub fn block_at(view: &dyn LiquidView, height: u32) -> Result<(Vec<u8>, Vec<Hash>)> {
    let hash = view.block_hash(height)?;
    Ok((view.header(&hash)?, view.txids(&hash)?))
}

/// The key of a mint behind the funding's adaptor point `T`, for the
/// user's L2 address `A` (x-only): `P = H(A ‖ T)·A + T`, x-only. She can
/// spend it once the funding shows `t`; the hub, never (its refund leaf
/// aside). Her secret: `H(A ‖ T)·a + t`, negated if `P`'s y is odd.
pub fn mint_key(l2_address: &Hash, mint_point: &PublicKey) -> Result<XOnlyPublicKey> {
    let secp = Secp256k1::verification_only();
    let a = XOnlyPublicKey::from_slice(l2_address).context("the L2 address isn't a key")?.public_key(bitcoin::secp256k1::Parity::Even);
    let h: [u8; 32] = Sha256::new().chain_update(b"lexit/mint").chain_update(l2_address).chain_update(mint_point.serialize()).finalize().into();
    let ha = a.mul_tweak(&secp, &bitcoin::secp256k1::Scalar::from_be_bytes(h).context("a tweak out of range")?)?;
    Ok(ha.combine(mint_point)?.x_only_public_key().0)
}

/// The secret of the mint key ([`mint_key`]), from the user's L2 key `a`
/// and the adaptor secret `t` the funding shows: `H(A ‖ T)·a + t`, `a`
/// negated if `A`'s y is odd. (A spend negates it again if `P`'s is.)
pub fn mint_secret(a: &bitcoin::secp256k1::Keypair, t: &bitcoin::secp256k1::SecretKey) -> Result<bitcoin::secp256k1::SecretKey> {
    let secp = Secp256k1::signing_only();
    let (l2, parity) = a.x_only_public_key();
    let mint_point = PublicKey::from_secret_key(&secp, t);
    let h: [u8; 32] = Sha256::new().chain_update(b"lexit/mint").chain_update(l2.serialize()).chain_update(mint_point.serialize()).finalize().into();
    let mut sk = a.secret_key();
    if parity == bitcoin::secp256k1::Parity::Odd {
        sk = sk.negate();
    }
    Ok(sk.mul_tweak(&bitcoin::secp256k1::Scalar::from_be_bytes(h)?)?.add_tweak(&bitcoin::secp256k1::Scalar::from(*t))?)
}

/// The mint's refund leaf: after `timeout` Liquid blocks, the hub's key
/// (`hub`, x-only), for a mint whose funding never came.
pub fn mint_refund_script(hub: &Hash, timeout: u16) -> elements::Script {
    elements::script::Builder::new()
        .push_int(i64::from(timeout))
        .push_opcode(elements::opcodes::all::OP_CSV)
        .push_opcode(elements::opcodes::all::OP_DROP)
        .push_slice(hub)
        .push_opcode(elements::opcodes::all::OP_CHECKSIG)
        .into_script()
}

/// The mint's Taproot tree: its internal key the mint key ([`mint_key`],
/// the user's once the funding shows `t`), one leaf the hub's refund
/// ([`mint_refund_script`]).
pub fn mint_tree(l2_address: &Hash, mint_point: &PublicKey, hub: &Hash, net: LiquidNet) -> Result<(elements::schnorr::UntweakedPublicKey, elements::taproot::TaprootSpendInfo)> {
    let secp = elements::secp256k1_zkp::Secp256k1::verification_only();
    let p = elements::schnorr::UntweakedPublicKey::from_slice(&mint_key(l2_address, mint_point)?.serialize())?;
    let info = elements::taproot::TaprootBuilder::new()
        .add_leaf(0, mint_refund_script(hub, net.mint_timeout()))
        .map_err(|e| anyhow::anyhow!("the refund leaf: {e:?}"))?
        .finalize(&secp, p)
        .map_err(|e| anyhow::anyhow!("the mint's tree: {e:?}"))?;
    Ok((p, info))
}

/// The mint's address ([`mint_tree`]), blinded to the user's key.
pub fn mint_address(l2_address: &Hash, mint_point: &PublicKey, hub: &Hash, blinder: &PublicKey, net: LiquidNet) -> Result<elements::Address> {
    let secp = elements::secp256k1_zkp::Secp256k1::verification_only();
    let (p, info) = mint_tree(l2_address, mint_point, hub, net)?;
    let blinder = elements::secp256k1_zkp::PublicKey::from_slice(&blinder.serialize())?;
    Ok(elements::Address::p2tr(&secp, p, info.merkle_root(), Some(blinder), net.address_params()))
}

/// A key-path-only Taproot address on `net` for the output key `key`,
/// blinded to `blinder`.
pub fn p2tr_address(key: &XOnlyPublicKey, blinder: Option<&PublicKey>, net: LiquidNet) -> Result<elements::Address> {
    let key = elements::schnorr::XOnlyPublicKey::from_slice(&key.serialize())?;
    let tweaked = elements::schnorr::TweakedPublicKey::new(key);
    let blinder = blinder.map(|b| elements::secp256k1_zkp::PublicKey::from_slice(&b.serialize())).transpose()?;
    Ok(elements::Address::p2tr_tweaked(tweaked, blinder, net.address_params()))
}

/// The address of contract `c`'s returns (`D` its terms' hash): its
/// return key ([`lexit_statement::return_key`]), blinded to the user's
/// contract key (`AliceKeys::blinding`): she can always open her returns;
/// the hub learns their openings from her claim.
pub fn return_address(hub: &Hash, d: &Hash, c: u32, blinder: &PublicKey, net: LiquidNet) -> Result<elements::Address> {
    let k = lexit_statement::return_key(hub, d, c).context("the hub's key isn't a key")?;
    p2tr_address(&XOnlyPublicKey::from_slice(&k)?, Some(blinder), net)
}

/// The block signing script of the tip's block: what a hub's terms pin,
/// and the user checks against her own view.
pub fn signblockscript(view: &dyn LiquidView) -> Result<Vec<u8>> {
    let hash = view.block_hash(view.height()?)?;
    let b = view.header(&hash)?;
    let h = lexit_elements::header::parse(&b).context("the tip's header isn't dynafed")?;
    let s = h.current.signblockscript().context("the tip's header names no signing script")?;
    ensure!(s.len() == 34 && s[..2] == [0, 0x20], "the block signing script isn't P2WSH");
    Ok(s.to_vec())
}

/// An asset's generator (secp256k1-zkp's, serialized): what a contract's
/// terms pin, so the statement never hashes to the curve.
pub fn generator(asset: &Hash) -> [u8; 33] {
    let secp = elements::secp256k1_zkp::Secp256k1::new();
    elements::secp256k1_zkp::Generator::new_unblinded(&secp, elements::secp256k1_zkp::Tag::from(*asset)).serialize()
}

/// The chain's policy asset, as the node names it (`getsidechaininfo`).
pub fn node_policy_asset(rpc: &Rpc) -> Result<Hash> {
    internal(rpc.call("getsidechaininfo", serde_json::json!([]))?["pegged_asset"].as_str().context("the node's pegged asset")?)
}

/// The hub's refund of a mint whose funding never came: the mint's output
/// (`vout` of `mint_tx`, opened as `secrets`) spent by its refund leaf,
/// after the timeout (the input's sequence), signed by the hub's key
/// `hub_secret`; all of it but `fee` to `to`, blinded. The mint's tree is
/// the contract's (its L2 address, the funding's adaptor point); `genesis`
/// the chain's (Elements' sighash commits to it).
#[allow(clippy::too_many_arguments)]
pub fn refund_tx(
    mint_tx: &elements::Transaction,
    vout: u32,
    secrets: elements::TxOutSecrets,
    l2_address: &Hash,
    mint_point: &PublicKey,
    hub_secret: &bitcoin::secp256k1::SecretKey,
    net: LiquidNet,
    to: &elements::Address,
    fee: u64,
    genesis: elements::BlockHash,
) -> Result<elements::Transaction> {
    use elements::pset::{Input, Output, PartiallySignedTransaction};
    use elements::secp256k1_zkp::{Keypair, Message, Secp256k1};
    use elements::sighash::{Prevouts, SighashCache};
    use elements::taproot::{LeafVersion, TapLeafHash};

    let secp = Secp256k1::new();
    let keypair = Keypair::from_seckey_slice(&secp, &hub_secret.secret_bytes())?;
    let hub = keypair.x_only_public_key().0.serialize();
    let timeout = net.mint_timeout();
    let (_, info) = mint_tree(l2_address, mint_point, &hub, net)?;
    let script = mint_refund_script(&hub, timeout);
    let control = info.control_block(&(script.clone(), LeafVersion::default())).context("the refund leaf's control block")?;
    let txout = mint_tx.output.get(vout as usize).context("no such output")?.clone();
    ensure!(secrets.value > fee, "the mint, {} sats, doesn't cover the fee", secrets.value);

    let mut pset = PartiallySignedTransaction::new_v2();
    let mut input = Input::from_prevout(elements::OutPoint::new(mint_tx.txid(), vout));
    input.witness_utxo = Some(txout.clone());
    input.sequence = Some(elements::Sequence(u32::from(timeout)));
    pset.add_input(input);
    let blinder = to.blinding_pubkey.map(bitcoin::PublicKey::new);
    let mut out = Output::new_explicit(to.script_pubkey(), secrets.value - fee, secrets.asset, blinder);
    out.blinder_index = Some(0);
    pset.add_output(out);
    pset.add_output(Output::new_explicit(elements::Script::new(), fee, secrets.asset, None));
    let inputs = std::collections::HashMap::from([(0usize, secrets)]);
    pset.blind_last(&mut rand::thread_rng(), &secp, &inputs).map_err(|e| anyhow::anyhow!("blinding the refund: {e:?}"))?;
    let mut tx = pset.extract_tx()?;
    let leaf = TapLeafHash::from_script(&script, LeafVersion::default());
    let hash = SighashCache::new(&tx).taproot_script_spend_signature_hash(0, &Prevouts::All(&[txout]), leaf, elements::SchnorrSighashType::Default, genesis)?;
    let sig = secp.sign_schnorr(&Message::from_digest(hash.to_byte_array()), &keypair);
    tx.input[0].witness.script_witness = vec![sig.serialize().to_vec(), script.into_bytes(), control.serialize()].into();
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mint_secret_is_the_mint_keys() {
        let secp = Secp256k1::new();
        for n in 1u8..6 {
            let a = bitcoin::secp256k1::Keypair::from_seckey_slice(&secp, &[n; 32]).unwrap();
            let t = bitcoin::secp256k1::SecretKey::from_slice(&[n + 40; 32]).unwrap();
            let p = mint_key(&a.x_only_public_key().0.serialize(), &PublicKey::from_secret_key(&secp, &t)).unwrap();
            let s = mint_secret(&a, &t).unwrap();
            assert_eq!(s.x_only_public_key(&secp).0, p);
        }
    }

    #[test]
    fn regtest_and_liquid_addresses_differ_and_carry_the_blinder() {
        let secp = Secp256k1::new();
        let k = bitcoin::secp256k1::SecretKey::from_slice(&[9; 32]).unwrap();
        let b = PublicKey::from_secret_key(&secp, &bitcoin::secp256k1::SecretKey::from_slice(&[8; 32]).unwrap());
        let x = k.x_only_public_key(&secp).0;
        let l = p2tr_address(&x, Some(&b), LiquidNet::Liquid).unwrap();
        let r = p2tr_address(&x, Some(&b), LiquidNet::Regtest).unwrap();
        assert!(l.is_blinded() && r.is_blinded());
        assert_ne!(l.to_string(), r.to_string());
        assert_eq!(l.script_pubkey(), r.script_pubkey());
        assert_eq!(l.script_pubkey().as_bytes()[2..], x.serialize());
    }
}
