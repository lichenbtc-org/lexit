//! L-BTC held by a party, the user's wallet or the hub: an LWK wallet
//! ([`Purse`]: a single-sig CT descriptor, `elwpkh` with a SLIP-77 blinding
//! key, from its own mnemonic),
//! synced from Liquid's Esplora (LWK's client) or from a node
//! ([`NodeBackend`]: elementsd over RPC, the chain indexed here by script,
//! for regtest and for users who run a node).
//!
//! Beyond paying and receiving, the two moves a contract asks:
//! - sweeping the mint ([`Purse::sweep`]): an output not the wallet's, its
//!   key the contract's mint key (Taproot, key path), spent with LWK
//!   blinding the transaction and our signature on the input;
//! - paying a return (an ordinary payment, [`Purse::send`]), the wallet
//!   then opening it with the contract's blinding key.

use std::collections::HashMap;

use anyhow::{bail, ensure, Context, Result};
use lwk_common::{singlesig_desc, DescriptorBlindingKey, Network, Signer, Singlesig};
use lwk_signer::SwSigner;
use lwk_wollet::blocking::BlockchainBackend;
use lwk_wollet::elements::encode::{deserialize, serialize};
use lwk_wollet::elements::pset::PartiallySignedTransaction;
use lwk_wollet::elements::schnorr::TapTweak;
use lwk_wollet::elements::secp256k1_zkp::{Keypair, Message, Secp256k1};
use lwk_wollet::elements::sighash::{Prevouts, SighashCache};
use lwk_wollet::elements::{Address, AssetId, BlockHash, BlockHeader, OutPoint, SchnorrSighashType, Script, Transaction, TxOut, TxOutSecrets, Txid};
use lwk_wollet::{ExternalUtxo, History, Wollet, WolletDescriptor};

use crate::liquid::{LiquidNet, Rpc};

/// LWK's network for a deployment: Liquid's and its testnet's are fixed; a
/// regtest chain's are its policy asset and genesis.
pub fn network(net: LiquidNet, policy_asset: &[u8; 32], genesis: Option<BlockHash>) -> Result<Network> {
    Ok(match net {
        LiquidNet::Liquid => Network::Liquid,
        LiquidNet::Testnet => Network::TestnetLiquid,
        LiquidNet::Regtest => {
            let mut display = *policy_asset;
            display.reverse();
            let asset: AssetId = hex::encode(display).parse()?;
            let b = lwk_common::ElementsParamsBuilder::new().with_policy_asset(asset);
            let b = match genesis {
                Some(g) => b.with_genesis_hash(g),
                None => b,
            };
            Network::CustomElements(b.build().map_err(|e| anyhow::anyhow!("{e:?}"))?)
        }
    })
}

/// A new wallet's mnemonic (12 words).
pub fn new_mnemonic(network: Network) -> Result<String> {
    let (_, m) = SwSigner::random_with_network(network).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(m.to_string())
}

/// elementsd as LWK's backend: blocks read from the node and indexed by
/// script (outputs paying it, inputs spending them) from `from` up, and
/// the mempool's transactions, read afresh at each scan (as an Esplora
/// reports them: so that a payment can follow an unconfirmed one).
pub struct NodeBackend {
    pub rpc: Rpc,
    scanned: Option<u32>,
    from: u32,
    by_script: HashMap<Script, Vec<(Txid, u32, BlockHash)>>,
    spk_of: HashMap<OutPoint, Script>,
    mempool: HashMap<Script, Vec<Txid>>,
}

impl NodeBackend {
    pub fn new(rpc: Rpc, from: u32) -> NodeBackend {
        NodeBackend { rpc, scanned: None, from, by_script: HashMap::new(), spk_of: HashMap::new(), mempool: HashMap::new() }
    }

    fn block_hash(&self, h: u32) -> Result<BlockHash> {
        Ok(self.rpc.call("getblockhash", serde_json::json!([h]))?.as_str().context("a block hash")?.parse()?)
    }

    /// Index the blocks not yet read.
    pub fn catch_up(&mut self) -> Result<()> {
        let tip = self.rpc.call("getblockcount", serde_json::json!([]))?.as_u64().context("a height")? as u32;
        let start = self.scanned.map_or(self.from, |s| s + 1);
        for h in start..=tip {
            let hash = self.block_hash(h)?;
            let raw = hex::decode(self.rpc.call("getblock", serde_json::json!([hash.to_string(), 0]))?.as_str().context("a block's hex")?)?;
            let block: lwk_wollet::elements::Block = deserialize(&raw)?;
            for tx in &block.txdata {
                let txid = tx.txid();
                for i in &tx.input {
                    if let Some(spk) = self.spk_of.get(&i.previous_output) {
                        self.by_script.entry(spk.clone()).or_default().push((txid, h, hash));
                    }
                }
                for (vout, o) in tx.output.iter().enumerate() {
                    if o.script_pubkey.is_empty() {
                        continue;
                    }
                    self.spk_of.insert(OutPoint::new(txid, vout as u32), o.script_pubkey.clone());
                    self.by_script.entry(o.script_pubkey.clone()).or_default().push((txid, h, hash));
                }
            }
            self.scanned = Some(h);
        }
        // the mempool, afresh: its outputs, and its inputs spending outputs
        // indexed (confirmed, or the mempool's own)
        self.mempool.clear();
        let mut spk_of: HashMap<OutPoint, Script> = HashMap::new();
        let ids = self.rpc.call("getrawmempool", serde_json::json!([]))?;
        let txs: Vec<Transaction> = ids
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| self.rpc.call("getrawtransaction", serde_json::json!([t])).ok())
            .filter_map(|h| hex::decode(h.as_str().unwrap_or_default()).ok().and_then(|b| deserialize(&b).ok()))
            .collect();
        for tx in &txs {
            for (vout, o) in tx.output.iter().enumerate() {
                if !o.script_pubkey.is_empty() {
                    spk_of.insert(OutPoint::new(tx.txid(), vout as u32), o.script_pubkey.clone());
                    self.mempool.entry(o.script_pubkey.clone()).or_default().push(tx.txid());
                }
            }
        }
        for tx in &txs {
            for i in &tx.input {
                if let Some(spk) = self.spk_of.get(&i.previous_output).or_else(|| spk_of.get(&i.previous_output)) {
                    self.mempool.entry(spk.clone()).or_default().push(tx.txid());
                }
            }
        }
        Ok(())
    }
}

fn lwk_err(e: impl std::fmt::Display) -> lwk_wollet::Error {
    lwk_wollet::Error::Generic(e.to_string())
}

impl BlockchainBackend for NodeBackend {
    fn tip(&mut self) -> Result<BlockHeader, lwk_wollet::Error> {
        self.catch_up().map_err(lwk_err)?;
        let hash = self.rpc.call("getbestblockhash", serde_json::json!([])).map_err(lwk_err)?;
        let h = self.rpc.call("getblockheader", serde_json::json!([hash, false])).map_err(lwk_err)?;
        deserialize(&hex::decode(h.as_str().unwrap_or_default()).map_err(lwk_err)?).map_err(lwk_err)
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, lwk_wollet::Error> {
        let t = self.rpc.call("sendrawtransaction", serde_json::json!([hex::encode(serialize(tx))])).map_err(lwk_err)?;
        t.as_str().unwrap_or_default().parse().map_err(lwk_err)
    }
    fn get_transactions(&self, txids: &[Txid]) -> Result<Vec<Transaction>, lwk_wollet::Error> {
        txids
            .iter()
            .map(|t| {
                let hex = self.rpc.call("getrawtransaction", serde_json::json!([t.to_string()])).map_err(lwk_err)?;
                deserialize(&hex::decode(hex.as_str().unwrap_or_default()).map_err(lwk_err)?).map_err(lwk_err)
            })
            .collect()
    }
    fn get_headers(&self, heights: &[u32], known: &HashMap<u32, BlockHash>) -> Result<Vec<BlockHeader>, lwk_wollet::Error> {
        heights
            .iter()
            .map(|h| {
                let hash = match known.get(h) {
                    Some(b) => *b,
                    None => self.block_hash(*h).map_err(lwk_err)?,
                };
                let hex = self.rpc.call("getblockheader", serde_json::json!([hash.to_string(), false])).map_err(lwk_err)?;
                deserialize(&hex::decode(hex.as_str().unwrap_or_default()).map_err(lwk_err)?).map_err(lwk_err)
            })
            .collect()
    }
    fn get_scripts_history(&self, scripts: &[&Script]) -> Result<Vec<Vec<History>>, lwk_wollet::Error> {
        Ok(scripts
            .iter()
            .map(|s| {
                let mut seen = std::collections::HashSet::new();
                let confirmed = self
                    .by_script
                    .get(*s)
                    .into_iter()
                    .flatten()
                    .filter(|(t, _, _)| seen.insert(*t))
                    .map(|(txid, h, hash)| History { txid: *txid, height: *h as i32, block_hash: Some(*hash), block_timestamp: None, v: 0 })
                    .collect::<Vec<_>>();
                // unconfirmed: -1, its parents perhaps unconfirmed too
                let unconfirmed = self.mempool.get(*s).into_iter().flatten().filter(|t| seen.insert(**t)).map(|t| History { txid: *t, height: -1, block_hash: None, block_timestamp: None, v: 0 });
                confirmed.into_iter().chain(unconfirmed).collect()
            })
            .collect())
    }
}

/// Where the purse syncs from: LWK's Esplora client, or a node.
pub enum Backend {
    Esplora(Box<lwk_wollet::blocking::EsploraClient>),
    Node(Box<NodeBackend>),
}

impl Backend {
    fn full_scan(&mut self, w: &Wollet) -> Result<Option<lwk_wollet::Update>> {
        Ok(match self {
            Backend::Esplora(c) => c.full_scan(w)?,
            Backend::Node(n) => {
                // LWK asks scripts' histories before the tip: the index
                // caught up first
                n.catch_up()?;
                n.full_scan(w)?
            }
        })
    }
    pub fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        Ok(match self {
            Backend::Esplora(c) => c.broadcast(tx)?,
            Backend::Node(n) => n.broadcast(tx)?,
        })
    }
}

/// The wallet's L-BTC.
pub struct Purse {
    pub wollet: Wollet,
    signer: SwSigner,
    pub network: Network,
    pub backend: Backend,
}

impl Purse {
    pub fn new(mnemonic: &str, network: Network, backend: Backend) -> Result<Purse> {
        let signer = SwSigner::new_with_network(mnemonic, network).map_err(|e| anyhow::anyhow!("the mnemonic: {e:?}"))?;
        let desc = singlesig_desc(&signer, Singlesig::Wpkh, DescriptorBlindingKey::Slip77).map_err(|e| anyhow::anyhow!("{e}"))?;
        let desc: WolletDescriptor = desc.parse()?;
        Ok(Purse { wollet: lwk_wollet::WolletBuilder::new(network, desc).build()?, signer, network, backend })
    }

    /// Catch up with the chain.
    pub fn sync(&mut self) -> Result<()> {
        if let Some(u) = self.backend.full_scan(&self.wollet)? {
            self.wollet.apply_update(u)?;
        }
        Ok(())
    }

    /// The L-BTC held, in sats.
    pub fn balance(&self) -> Result<u64> {
        Ok(self.wollet.balance()?.get(self.network.policy_asset()).copied().unwrap_or(0))
    }

    /// An address to receive at (the first unused).
    pub fn address(&self) -> Result<Address> {
        Ok(self.wollet.address(None)?.address().clone())
    }

    /// Pay `sats` of L-BTC to `to`: the transaction, broadcast.
    pub fn send(&mut self, to: &Address, sats: u64) -> Result<Transaction> {
        let mut pset = self.wollet.tx_builder().add_lbtc_recipient(to, sats)?.finish()?;
        self.sign_finish(&mut pset)
    }

    /// Sweep outputs of ours that aren't the wallet's, each a Taproot
    /// key-path spend by its keypair (the output key's: already tweaked,
    /// for a key with a tree), each given with its output and its opening:
    /// everything to the wallet, the fee out of it.
    pub fn sweep(&mut self, inputs: Vec<(OutPoint, TxOut, TxOutSecrets, Keypair)>) -> Result<Transaction> {
        ensure!(!inputs.is_empty(), "nothing to sweep");
        let utxos = inputs.iter().map(|(outpoint, txout, secrets, _)| ExternalUtxo { outpoint: *outpoint, txout: txout.clone(), tx: None, unblinded: *secrets, max_weight_to_satisfy: 66 }).collect();
        let mut pset = self.wollet.tx_builder().add_external_utxos(utxos)?.drain_lbtc_wallet().finish()?;
        let secp = Secp256k1::new();
        let prevouts: Vec<TxOut> = pset.inputs().iter().map(|i| i.witness_utxo.clone().context("an input's previous output")).collect::<Result<_>>()?;
        let tx = pset.extract_tx()?;
        let genesis = self.network.genesis_hash();
        let mut cache = SighashCache::new(&tx);
        for (outpoint, _, _, keypair) in &inputs {
            let i = pset.inputs().iter().position(|i| i.previous_txid == outpoint.txid && i.previous_output_index == outpoint.vout).context("a swept output among the inputs")?;
            let hash = cache.taproot_key_spend_signature_hash(i, &Prevouts::All(&prevouts), SchnorrSighashType::Default, genesis)?;
            let sig = secp.sign_schnorr(&Message::from_digest(hash.to_byte_array()), keypair);
            pset.inputs_mut()[i].tap_key_sig = Some(lwk_wollet::elements::SchnorrSig { sig, hash_ty: SchnorrSighashType::Default });
        }
        self.sign_finish(&mut pset)
    }

    /// A key-path keypair for an output with a tree: `keypair` (the
    /// internal key's) tweaked by the tree's root.
    pub fn tweaked(keypair: &Keypair, merkle_root: Option<lwk_wollet::elements::taproot::TapNodeHash>) -> Keypair {
        keypair.tap_tweak(&Secp256k1::new(), merkle_root).to_inner()
    }

    /// The wallet's inputs signed, finalized, broadcast, and applied.
    fn sign_finish(&mut self, pset: &mut PartiallySignedTransaction) -> Result<Transaction> {
        self.signer.sign(pset).map_err(|e| anyhow::anyhow!("signing: {e:?}"))?;
        let tx = self.wollet.finalize(pset)?;
        self.backend.broadcast(&tx)?;
        self.wollet.apply_transaction(tx.clone())?;
        Ok(tx)
    }
}

/// The opening of output `vout` of `tx` by `blinding` (secret key bytes):
/// value and blinding factors, as a claim takes them (big-endian scalars),
/// checked to open its commitments under `generator`.
pub fn open(tx: &Transaction, vout: u32, blinding: &[u8; 32], generator: &[u8; 33]) -> Result<(u64, [u8; 32], [u8; 32], TxOutSecrets)> {
    let secp = Secp256k1::new();
    let o = tx.output.get(vout as usize).context("no such output")?;
    let sk = lwk_wollet::elements::secp256k1_zkp::SecretKey::from_slice(blinding)?;
    let s = o.unblind(&secp, sk).map_err(|e| anyhow::anyhow!("the output doesn't unblind with our key: {e:?}"))?;
    let (asset, value) = (serialize(&o.asset), serialize(&o.value));
    ensure!(asset.len() == 33 && value.len() == 33, "the output isn't confidential");
    let (a, v): ([u8; 33], [u8; 33]) = (asset.try_into().unwrap(), value.try_into().unwrap());
    for reverse in [false, true] {
        let mut r_a: [u8; 32] = *s.asset_bf.into_inner().as_ref();
        let mut r_v: [u8; 32] = *s.value_bf.into_inner().as_ref();
        if reverse {
            r_a.reverse();
            r_v.reverse();
        }
        if lexit_elements::commit::opens(&a, &v, generator, s.value, &r_a, &r_v) {
            return Ok((s.value, r_a, r_v, s));
        }
    }
    bail!("the output's blinding factors don't open its commitments")
}
