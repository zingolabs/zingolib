use std::{
    fs::{self, File},
    io::{self, BufRead},
    path::{Path, PathBuf},
    process::{Child, Command},
    time::Duration,
};

use http::Uri;
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use tempdir;
use zcash_primitives::merkle_tree::read_commitment_tree;

use zcash_primitives::sapling::{note_encryption::SaplingDomain, Node};
use zingo_testutils::{
    self,
    incrementalmerkletree::frontier::CommitmentTree,
    regtest::{get_regtest_dir, launch_lightwalletd},
};
use zingolib::wallet::traits::DomainWalletExt;

use super::{
    constants,
    darkside_types::{RawTransaction, TreeState},
};

use crate::darkside::darkside_connector::DarksideConnector;

pub fn generate_darksidewalletd() -> (String, PathBuf) {
    let darkside_grpc_port = portpicker::pick_unused_port()
        .expect("Port unpickable!")
        .to_string();
    let darkside_dir = tempdir::TempDir::new("zingo_darkside_test")
        .unwrap()
        .into_path();
    (darkside_grpc_port, darkside_dir)
}

pub struct DarksideHandler {
    pub lightwalletd_handle: Child,
    pub grpc_port: String,
    pub darkside_dir: PathBuf,
}

impl DarksideHandler {
    pub fn new() -> Self {
        let (grpc_port, darkside_dir) = generate_darksidewalletd();
        let grpc_bind_addr = Some(format!("127.0.0.1:{grpc_port}"));
        let darkside_dir_string = darkside_dir.to_string_lossy().to_string();
        println!("Darksidewalletd running at {darkside_dir_string}");

        let check_interval = Duration::from_millis(300);
        let lightwalletd_handle = launch_lightwalletd(
            darkside_dir.join("logs"),
            darkside_dir.join("conf"),
            darkside_dir.join("data"),
            get_regtest_dir().join("bin"),
            check_interval,
            grpc_bind_addr,
        );
        Self {
            lightwalletd_handle,
            grpc_port,
            darkside_dir,
        }
    }
}

impl Drop for DarksideHandler {
    fn drop(&mut self) {
        if let Err(_) = Command::new("kill")
            .arg(self.lightwalletd_handle.id().to_string())
            .output()
        {
            // if regular kill doesn't work, kill it harder
            let _ = self.lightwalletd_handle.kill();
        }
    }
}

pub(crate) async fn update_tree_states_for_transaction(
    server_id: &Uri,
    raw_tx: RawTransaction,
    height: u64,
) -> TreeState {
    let trees = zingolib::grpc_connector::GrpcConnector::get_trees(server_id.clone(), height - 1)
        .await
        .unwrap();
    let mut sapling_tree: zcash_primitives::sapling::CommitmentTree = read_commitment_tree(
        hex::decode(SaplingDomain::get_tree(&trees))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let mut orchard_tree: CommitmentTree<MerkleHashOrchard, 32> = read_commitment_tree(
        hex::decode(OrchardDomain::get_tree(&trees))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let transaction = zcash_primitives::transaction::Transaction::read(
        raw_tx.data.as_slice(),
        zcash_primitives::consensus::BranchId::Nu5,
    )
    .unwrap();
    for output in transaction
        .sapling_bundle()
        .iter()
        .flat_map(|bundle| bundle.shielded_outputs())
    {
        sapling_tree.append(Node::from_cmu(output.cmu())).unwrap()
    }
    for action in transaction
        .orchard_bundle()
        .iter()
        .flat_map(|bundle| bundle.actions())
    {
        orchard_tree
            .append(MerkleHashOrchard::from_cmx(action.cmx()))
            .unwrap()
    }
    let mut sapling_tree_bytes = vec![];
    zcash_primitives::merkle_tree::write_commitment_tree(&sapling_tree, &mut sapling_tree_bytes)
        .unwrap();
    let mut orchard_tree_bytes = vec![];
    zcash_primitives::merkle_tree::write_commitment_tree(&orchard_tree, &mut orchard_tree_bytes)
        .unwrap();
    let new_tree_state = TreeState {
        height,
        sapling_tree: hex::encode(sapling_tree_bytes),
        orchard_tree: hex::encode(orchard_tree_bytes),
        network: constants::first_tree_state().network,
        hash: "".to_string(),
        time: 0,
    };
    DarksideConnector::new(server_id.clone())
        .add_tree_state(new_tree_state.clone())
        .await
        .unwrap();
    new_tree_state
}

// The output is wrapped in a Result to allow matching on errors
// Returns an Iterator to the Reader of the lines of the file.
// source: https://doc.rust-lang.org/rust-by-example/std_misc/file/read_lines.html
pub(crate) fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

pub(crate) fn read_block_dataset<P>(filename: P) -> Vec<String>
where
    P: AsRef<Path>,
{
    read_lines(filename)
        .unwrap()
        .map(|line| line.unwrap())
        .collect()
}

impl TreeState {
    pub(crate) fn from_file<P>(filename: P) -> Result<TreeState, Box<dyn std::error::Error>>
    where
        P: AsRef<Path>,
    {
        let state_string = fs::read_to_string(filename)?;
        let json_state: serde_json::Value = serde_json::from_str(&state_string)?;

        let network = json_state["network"].as_str().unwrap();
        let height = json_state["height"].as_u64().unwrap();
        let hash = json_state["hash"].as_str().unwrap();
        let time = json_state["time"].as_i64().unwrap();
        let time_32: u32 = u32::try_from(time)?;
        let sapling_tree = json_state["saplingTree"].as_str().unwrap();
        let orchard_tree = json_state["orchardTree"].as_str().unwrap();

        Ok(TreeState {
            network: network.to_string(),
            height: height,
            hash: hash.to_string(),
            time: time_32,
            sapling_tree: sapling_tree.to_string(),
            orchard_tree: orchard_tree.to_string(),
        })
    }
}
