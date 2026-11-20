//! A throwaway Elements regtest chain for tests and the devstack: Liquid's
//! shape (signed blocks, dynamic federations from the first block) with a
//! federation of our own ([`Federation`]: an `m`-of-`n` multisig whose keys
//! we hold), so that tests can make blocks, and also forge or withhold
//! them.
//!
//! `elementsd` is `LEXIT_ELEMENTSD`'s, or the one on the PATH. Blocks are
//! the node's (`getnewblockhex`), signed here and submitted
//! ([`ElementsRegtest::mine`]). The genesis block's free coins
//! (`-initialfreecoins`, anyone-can-spend) are the `harness` wallet's.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use lexit_elements::{header, sha256};
use sha2::{Digest, Sha256};

use crate::liquid::Rpc;

/// A federation: `m` of `keys` sign each block.
#[derive(Clone)]
pub struct Federation {
    pub keys: Vec<SecretKey>,
    pub m: usize,
}

impl Federation {
    /// The tests' 2-of-3, its keys from labels (anyone can derive them).
    pub fn regtest() -> Federation {
        let keys = (0..3u8).map(|i| SecretKey::from_slice(&Sha256::digest([b"lexit/regtest-federation/".as_slice(), &[i]].concat())).unwrap()).collect();
        Federation { keys, m: 2 }
    }
    fn pubkeys(&self) -> Vec<PublicKey> {
        let secp = Secp256k1::signing_only();
        self.keys.iter().map(|k| PublicKey::from_secret_key(&secp, k)).collect()
    }
    /// The multisig, `OP_m <key>… OP_n OP_CHECKMULTISIG`: the node's
    /// `-signblockscript`, and the witness script.
    pub fn witness_script(&self) -> Vec<u8> {
        let mut s = vec![0x50 + self.m as u8];
        for k in self.pubkeys() {
            s.push(33);
            s.extend(k.serialize());
        }
        s.extend([0x50 + self.keys.len() as u8, 0xae]);
        s
    }
    /// The chain's block signing script: P2WSH of the multisig.
    pub fn signblockscript(&self) -> Vec<u8> {
        [&[0x00, 0x20][..], &sha256(&self.witness_script())].concat()
    }
    /// `block` (serialized, its header's witness empty) signed by the
    /// first `m` keys.
    pub fn sign_block(&self, block: &[u8]) -> Result<Vec<u8>> {
        let (h, len) = header::parse_prefix(block).context("a block starting with a dynafed header")?;
        ensure!(h.witness.is_empty(), "the block is signed already");
        let msg = Message::from_digest(h.hash());
        let secp = Secp256k1::signing_only();
        let mut stack: Vec<Vec<u8>> = vec![vec![]];
        for k in &self.keys[..self.m] {
            let mut sig = secp.sign_ecdsa(&msg, k).serialize_der().to_vec();
            sig.push(0x01);
            stack.push(sig);
        }
        stack.push(self.witness_script());
        let mut out = h.unsigned.to_vec();
        out.push(stack.len() as u8);
        for item in &stack {
            ensure!(item.len() < 0xfd, "a witness item of {} bytes", item.len());
            out.push(item.len() as u8);
            out.extend(item);
        }
        out.extend(&block[len..]);
        Ok(out)
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// An `elementsd -chain=elementsregtest` of our own, stopped when dropped.
pub struct ElementsRegtest {
    child: Child,
    pub datadir: PathBuf,
    pub federation: Federation,
    /// The node, its `harness` wallet holding the free coins.
    pub rpc: Rpc,
}

impl ElementsRegtest {
    /// Start a node with `federation`, and sign its first block.
    pub fn start(federation: Federation) -> Result<ElementsRegtest> {
        let bin = std::env::var("LEXIT_ELEMENTSD").unwrap_or_else(|_| "elementsd".into());
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let datadir = std::env::temp_dir().join(format!("lexit-elements-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&datadir);
        std::fs::create_dir_all(&datadir)?;
        let rpc_port = free_port();
        let child = Command::new(&bin)
            .args([
                "-chain=elementsregtest",
                "-server=1",
                "-listen=0",
                "-txindex=1",
                "-con_signed_blocks=1",
                &format!("-signblockscript={}", hex::encode(federation.witness_script())),
                // dynamic federations from the first block: Liquid's headers
                "-evbparams=dynafed:-1:::",
                // the signatures' room (regtest's default, 74 bytes, fits one
                // signature; Liquid's is 1,325)
                "-con_max_block_sig_size=1325",
                "-initialfreecoins=2100000000000000",
                "-anyonecanspendaremine=1",
                "-validatepegin=0",
                "-fallbackfee=0.0001",
                "-acceptnonstdtxn=1",
                "-debuglogfile=debug.log",
                &format!("-datadir={}", datadir.display()),
                &format!("-rpcport={rpc_port}"),
                &format!("-port={}", free_port()),
                "-rpcbind=127.0.0.1",
                "-rpcallowip=127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning {bin} (LEXIT_ELEMENTSD names another)"))?;
        let url = format!("http://127.0.0.1:{rpc_port}");
        let cookie = datadir.join("elementsregtest").join(".cookie");
        let node = ElementsRegtest { child, datadir, federation, rpc: Rpc { url, cookie, wallet: "harness".into() } };
        let deadline = Instant::now() + Duration::from_secs(60);
        while !(node.rpc.cookie.exists() && node.rpc.call("getblockcount", serde_json::json!([])).is_ok()) {
            if Instant::now() > deadline {
                bail!("elementsd did not come up within 60 s ({})", node.datadir.display());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        node.rpc.call("createwallet", serde_json::json!(["harness"]))?;
        node.rpc.wallet_call("rescanblockchain", serde_json::json!([]))?;
        node.mine(1)?;
        Ok(node)
    }

    /// The node's RPC for wallet `name` (made if new).
    pub fn wallet(&self, name: &str) -> Result<Rpc> {
        let w = Rpc { url: self.rpc.url.clone(), cookie: self.rpc.cookie.clone(), wallet: name.into() };
        let _ = self.rpc.call("createwallet", serde_json::json!([name]));
        Ok(w)
    }

    /// `n` blocks, each the node's, signed by the federation and
    /// submitted.
    pub fn mine(&self, n: u32) -> Result<()> {
        for _ in 0..n {
            let b = hex::decode(self.rpc.call("getnewblockhex", serde_json::json!([]))?.as_str().context("a block's hex")?)?;
            let signed = self.federation.sign_block(&b)?;
            let r = self.rpc.call("submitblock", serde_json::json!([hex::encode(signed)]))?;
            ensure!(r.is_null(), "the node refused our block: {r}");
        }
        Ok(())
    }
}

impl Drop for ElementsRegtest {
    fn drop(&mut self) {
        let _ = self.rpc.call("stop", serde_json::json!([]));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        if std::env::var("LEXIT_KEEP_DATADIR").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.datadir);
        }
    }
}
