//! Opt-in (`--ignored`, needs the network): a hub on Liquid testnet with no
//! node, reading Blockstream's public Esplora (its LWK wallet synced
//! through Blockstream's Waterfalls). It opens (its keys made, its wallet
//! synced), names testnet's L-BTC, and takes the
//! federation's block signing script from the tip; the tip's header, as
//! the Esplora serves it, is signed under that script as the statement
//! checks it, and its transactions are its root's.

use lexit_elements::{header, merkle};
use lexit_hub::{LiquidHub, LiquidSource};
use lexit_proto::liquid::{Esplora, LiquidNet, LiquidView};

#[test]
#[ignore]
fn a_hub_reads_liquid_testnet_from_its_esplora() {
    let dir = std::env::temp_dir().join(format!("lexit-esplora-testnet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let url = LiquidNet::Testnet.default_esplora().unwrap().to_string();
    let waterfalls = LiquidNet::Testnet.default_waterfalls().unwrap().to_string();
    let hub = LiquidHub::open(&dir, LiquidNet::Testnet, LiquidSource::EsploraWaterfalls { esplora: url.clone(), waterfalls }, lichen_proto::Pricing::default()).unwrap();
    assert_eq!(Some(hub.deployment.asset), LiquidNet::Testnet.policy_asset());
    assert_eq!(hub.balance().unwrap(), 0, "a new wallet");
    assert!(hub.address().unwrap().is_blinded());

    let view = Esplora { url };
    let tip = view.height().unwrap();
    let hash = view.block_hash(tip).unwrap();
    let b = view.header(&hash).unwrap();
    let h = header::parse(&b).expect("a dynafed header");
    assert_eq!(h.height, tip);
    assert!(header::signed(&h, &hub.deployment.signblockscript), "the tip signed under the federation's script");
    let txids = view.txids(&hash).unwrap();
    assert_eq!(merkle::root(&txids), Some(h.merkle_root), "its transactions are its root's");
    println!("Liquid testnet: tip {tip}, {} transactions; the federation's script {}", txids.len(), hex::encode(&hub.deployment.signblockscript));
    let _ = std::fs::remove_dir_all(&dir);
}
