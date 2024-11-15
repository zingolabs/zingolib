use http::Uri;
use hyper_util::client::legacy::connect::HttpConnector;
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use sapling_crypto::note_encryption::SaplingDomain;
use std::{
    fs,
    fs::File,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
    time::Duration,
};
use tempdir;
use tokio::time::sleep;
use tower::ServiceExt;
use zcash_primitives::consensus::BranchId;
use zcash_primitives::{merkle_tree::read_commitment_tree, transaction::Transaction};
use zingo_netutils::UnderlyingService;
use zingolib::testutils::{
    incrementalmerkletree::frontier::CommitmentTree,
    paths::{get_bin_dir, get_cargo_manifest_dir},
    regtest::launch_lightwalletd,
    scenarios::setup::TestEnvironmentGenerator,
};
use zingolib::wallet::traits::DomainWalletExt;

use crate::{
    constants::BRANCH_ID,
    darkside_types::{
        self, darkside_streamer_client::DarksideStreamerClient, DarksideBlock, DarksideBlocksUrl,
        DarksideEmptyBlocks, DarksideHeight, DarksideMetaState, Empty,
    },
};

use super::{
    constants,
    darkside_types::{RawTransaction, TreeState},
};
macro_rules! define_darkside_connector_methods(
    ($($name:ident (&$self:ident $(,$param:ident: $param_type:ty)*$(,)?) -> $return:ty {$param_packing:expr}),*) => {$(
        #[allow(unused)]
        pub async fn $name(&$self, $($param: $param_type),*) -> ::std::result::Result<$return, String> {
            let request = ::tonic::Request::new($param_packing);

            let mut client = $self.get_client().await.map_err(|e| format!("{e}"))?;

        client
            .$name(request)
            .await
            .map_err(|e| format!("{}", e))
            .map(|resp| resp.into_inner())
    })*}
);

#[derive(Clone)]
pub struct DarksideConnector(pub http::Uri);

impl DarksideConnector {
    pub fn get_client(
        &self,
    ) -> impl std::future::Future<
        Output = Result<DarksideStreamerClient<UnderlyingService>, Box<dyn std::error::Error>>,
    > {
        let uri = Arc::new(self.0.clone());
        async move {
            let mut http_connector = HttpConnector::new();
            http_connector.enforce_http(false);
            let connector = tower::ServiceBuilder::new().service(http_connector);
            let client = zingo_netutils::client::client_from_connector(connector, true);
            let uri = uri.clone();
            let svc = tower::ServiceBuilder::new()
                //Here, we take all the pieces of our uri, and add in the path from the Requests's uri
                .map_request(move |mut req: http::Request<tonic::body::BoxBody>| {
                    let uri = Uri::builder()
                        .scheme(uri.scheme().unwrap().clone())
                        .authority(uri.authority().unwrap().clone())
                        //here. The Request's uri contains the path to the GRPC sever and
                        //the method being called
                        .path_and_query(req.uri().path_and_query().unwrap().clone())
                        .build()
                        .unwrap();

                    *req.uri_mut() = uri;
                    req
                })
                .service(client);

            Ok(DarksideStreamerClient::new(svc.boxed_clone()))
        }
    }

    define_darkside_connector_methods!(
        apply_staged(&self, height: i32) -> Empty { DarksideHeight { height } },
        add_tree_state(&self, tree_state: TreeState) -> Empty { tree_state },
        reset(
            &self,
            sapling_activation: i32,
            branch_id: String,
            chain_name: String,
        ) -> Empty {
            DarksideMetaState {
                sapling_activation,
                branch_id,
                chain_name,
            }
        },
        stage_blocks(&self, url: String) -> Empty { DarksideBlocksUrl { url } },
        stage_blocks_create(
            &self,
            height: i32,
            count: i32,
            nonce: i32
        ) -> Empty {
            DarksideEmptyBlocks {
                height,
                count,
                nonce
            }
        },

        stage_blocks_stream(&self, blocks: Vec<String>) -> Empty {
            ::futures_util::stream::iter(
                blocks.into_iter().map(|block| DarksideBlock { block })
            )
        },
        stage_transactions_stream(&self, transactions: Vec<(Vec<u8>, u64)>) -> Empty {
            ::futures_util::stream::iter(
                transactions.into_iter().map(|transaction| {
                    RawTransaction {
                        data: transaction.0,
                        height: transaction.1
                    }
                })
            )
        },
        get_incoming_transactions(&self) -> ::tonic::Streaming<RawTransaction> {
            Empty {}
        },
        clear_incoming_transactions(&self) -> Empty {
            Empty {}
        }
    );
}

pub async fn prepare_darksidewalletd(
    uri: http::Uri,
    include_startup_funds: bool,
) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(1, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    connector
        .stage_blocks_stream(vec![String::from(crate::constants::GENESIS_BLOCK)])
        .await?;

    connector.stage_blocks_create(2, 2, 0).await.unwrap();

    connector
        .add_tree_state(constants::first_tree_state())
        .await
        .unwrap();
    if include_startup_funds {
        connector
            .stage_transactions_stream(vec![(
                hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                2,
            )])
            .await
            .unwrap();
        let tree_height_2 = update_tree_states_for_transaction(
            &uri,
            RawTransaction {
                data: hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                height: 2,
            },
            2,
        )
        .await;
        connector
            .add_tree_state(TreeState {
                height: 3,
                ..tree_height_2
            })
            .await
            .unwrap();
    } else {
        for height in [2, 3] {
            connector
                .add_tree_state(TreeState {
                    height,
                    ..constants::first_tree_state()
                })
                .await
                .unwrap();
        }
    }

    sleep(std::time::Duration::new(2, 0)).await;

    connector.apply_staged(3).await?;

    Ok(())
}
pub fn generate_darksidewalletd(set_port: Option<portpicker::Port>) -> (String, PathBuf) {
    let darkside_grpc_port = TestEnvironmentGenerator::pick_unused_port_to_string(set_port);
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

impl Default for DarksideHandler {
    fn default() -> Self {
        Self::new(None)
    }
}
impl DarksideHandler {
    pub fn new(set_port: Option<portpicker::Port>) -> Self {
        let (grpc_port, darkside_dir) = generate_darksidewalletd(set_port);
        let grpc_bind_addr = Some(format!("127.0.0.1:{grpc_port}"));

        let check_interval = Duration::from_millis(300);
        let lightwalletd_handle = launch_lightwalletd(
            darkside_dir.join("logs"),
            darkside_dir.join("conf"),
            darkside_dir.join("data"),
            get_bin_dir(),
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
        if Command::new("kill")
            .arg(self.lightwalletd_handle.id().to_string())
            .output()
            .is_err()
        {
            // if regular kill doesn't work, kill it harder
            self.lightwalletd_handle
                .kill()
                .expect("command couldn't be killed");
        }
    }
}

/// Takes a raw transaction and then updates and returns tree state from the previous block
pub async fn update_tree_states_for_transaction(
    server_id: &Uri,
    raw_tx: RawTransaction,
    height: u64,
) -> TreeState {
    let trees = zingolib::grpc_connector::get_trees(server_id.clone(), height - 1)
        .await
        .unwrap();
    let mut sapling_tree: sapling_crypto::CommitmentTree = read_commitment_tree(
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
        sapling_tree
            .append(sapling_crypto::Node::from_cmu(output.cmu()))
            .unwrap()
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
    DarksideConnector(server_id.clone())
        .add_tree_state(new_tree_state.clone())
        .await
        .unwrap();
    new_tree_state
}

// The output is wrapped in a Result to allow matching on errors
// Returns an Iterator to the Reader of the lines of the file.
// source: https://doc.rust-lang.org/rust-by-example/std_misc/file/read_lines.html
pub fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

/// Tool for reading lists of blocks or transactions.
/// Takes path to file and returns each line in a vec of strings.
pub fn read_dataset<P>(filename: P) -> Vec<String>
where
    P: AsRef<Path>,
{
    read_lines(filename)
        .unwrap()
        .map(|line| line.unwrap())
        .collect()
}

impl TreeState {
    pub fn from_file<P>(filename: P) -> Result<TreeState, Box<dyn std::error::Error>>
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
            height,
            hash: hash.to_string(),
            time: time_32,
            sapling_tree: sapling_tree.to_string(),
            orchard_tree: orchard_tree.to_string(),
        })
    }
}

/// Basic initialisation of darksidewalletd.
/// Returns a darkside handler and darkside connector.
/// Generates a genesis block and adds initial treestate.
pub async fn init_darksidewalletd(
    set_port: Option<portpicker::Port>,
) -> Result<(DarksideHandler, DarksideConnector), String> {
    let handler = DarksideHandler::new(set_port);
    let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        handler.grpc_port
    )));
    let connector = DarksideConnector(server_id);

    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    let mut client = connector.get_client().await.unwrap();
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(1, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    // stage genesis block
    connector
        .stage_blocks_stream(vec![String::from(crate::constants::GENESIS_BLOCK)])
        .await?;
    connector
        .add_tree_state(constants::first_tree_state())
        .await
        .unwrap();

    Ok((handler, connector))
}

/// Creates a file for writing transactions to store pre-built blockchains.
/// Path: `darkside-tests/tests/data/chainbuilds/{test_name}`
/// For writing transactions, see `send_and_write_transaction` method in DarksideScenario.
pub fn create_chainbuild_file(test_name: &str) -> File {
    let path = format!(
        "{}/tests/data/chainbuilds/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        test_name
    );
    match fs::create_dir(path.clone()) {
        Ok(_) => (),
        Err(e) => match e.kind() {
            io::ErrorKind::AlreadyExists => (),
            _ => panic!("Error creating directory: {}", e),
        },
    }
    let filename = "hex_transactions.txt";
    fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(format!("{}/{}", path, filename))
        .expect("file should not already exist")
}
/// Loads a vec of strings from a list of hex transactions in the chainbuild file
/// Path: `darkside-tests/tests/data/chainbuilds/{test_name}`
/// For staging hex transactions, see `stage_transaction` method in DarksideScenario
pub fn load_chainbuild_file(test_name: &str) -> Vec<String> {
    let path = format!(
        "{}/tests/data/chainbuilds/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        test_name
    );
    let filename = "hex_transactions.txt";
    read_dataset(format!("{}/{}", path, filename))
}
/// Hex encodes raw transaction and writes to file.
fn write_raw_transaction(
    raw_transaction: &darkside_types::RawTransaction,
    branch_id: BranchId,
    chainbuild_file: &File,
) {
    let transaction = create_transaction_from_raw_transaction(raw_transaction, branch_id).unwrap();
    write_transaction(transaction, chainbuild_file);
}
/// Converts raw transaction to transaction.
fn create_transaction_from_raw_transaction(
    raw_transaction: &darkside_types::RawTransaction,
    branch_id: BranchId,
) -> Result<Transaction, io::Error> {
    Transaction::read(&raw_transaction.data[..], branch_id)
}
/// Hex encodes transaction and writes to file
fn write_transaction(transaction: Transaction, mut chainbuild_file: &File) {
    let mut buffer = vec![];
    let mut cursor = std::io::Cursor::new(&mut buffer);
    transaction
        .write(&mut cursor)
        .expect("transaction should be written to a buffer");
    let hex_transaction = hex::encode(buffer);
    chainbuild_file
        .write_all(format!("{}\n", hex_transaction).as_bytes())
        .unwrap();
}

pub mod scenarios {
    use std::fs::File;
    use std::ops::Add;

    use crate::{
        constants,
        darkside_types::{RawTransaction, TreeState},
    };
    use zcash_client_backend::{PoolType, ShieldedProtocol};
    use zcash_primitives::consensus::{BlockHeight, BranchId};
    use zingolib::config::RegtestNetwork;
    use zingolib::lightclient::LightClient;
    use zingolib::testutils::scenarios::setup::ClientBuilder;
    use zingolib::testvectors::seeds::HOSPITAL_MUSEUM_SEED;

    use super::{
        init_darksidewalletd, update_tree_states_for_transaction, write_raw_transaction,
        DarksideConnector, DarksideHandler,
    };

    pub struct DarksideEnvironment {
        darkside_handler: DarksideHandler,
        pub(crate) darkside_connector: DarksideConnector,
        pub(crate) client_builder: ClientBuilder,
        pub(crate) regtest_network: RegtestNetwork,
        faucet: Option<LightClient>,
        lightclients: Vec<LightClient>,
        pub(crate) staged_blockheight: BlockHeight,
        pub(crate) tree_state: TreeState,
        transaction_set_index: u64,
    }
    impl DarksideEnvironment {
        /// Initialises and launches darksidewalletd, stages the genesis block and creates the lightclient builder
        pub async fn new(set_port: Option<portpicker::Port>) -> DarksideEnvironment {
            let (darkside_handler, darkside_connector) =
                init_darksidewalletd(set_port).await.unwrap();
            let client_builder = ClientBuilder::new(
                darkside_connector.0.clone(),
                darkside_handler.darkside_dir.clone(),
            );
            let regtest_network = RegtestNetwork::all_upgrades_active();
            DarksideEnvironment {
                darkside_handler,
                darkside_connector,
                client_builder,
                regtest_network,
                faucet: None,
                lightclients: vec![],
                staged_blockheight: BlockHeight::from(1),
                tree_state: constants::first_tree_state(),
                transaction_set_index: 0,
            }
        }
        pub async fn default() -> DarksideEnvironment {
            DarksideEnvironment::new(None).await
        }
        pub async fn default_faucet_recipient(funded_pool: PoolType) -> DarksideEnvironment {
            let mut scenario = DarksideEnvironment::new(None).await;
            scenario
                .build_faucet(funded_pool)
                .await
                .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 4)
                .await;
            scenario
        }

        /// Builds a lightclient with spending capability to the initial source of funds to the darkside blockchain
        /// The staged block with the funding transaction is not applied and the faucet is not synced
        pub async fn build_faucet(&mut self, funded_pool: PoolType) -> &mut DarksideEnvironment {
            if self.faucet.is_some() {
                panic!("Error: Faucet already exists!");
            }
            self.faucet = Some(
                self.client_builder
                    .build_client(
                        zingolib::testvectors::seeds::DARKSIDE_SEED.to_string(),
                        0,
                        true,
                        self.regtest_network,
                    )
                    .await,
            );

            let faucet_funding_transaction = match funded_pool {
                PoolType::Shielded(ShieldedProtocol::Orchard) => {
                    constants::ABANDON_TO_DARKSIDE_ORCH_10_000_000_ZAT
                }
                PoolType::Shielded(ShieldedProtocol::Sapling) => {
                    constants::ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT
                }
                PoolType::Transparent => {
                    panic!("Error: Transparent funding transactions for faucet are not currently implemented!")
                }
            };
            self.stage_transaction(faucet_funding_transaction).await;
            self
        }
        /// Builds a new lightclient from a seed phrase
        pub async fn build_client(
            &mut self,
            seed: String,
            birthday: u64,
        ) -> &mut DarksideEnvironment {
            let lightclient = self
                .client_builder
                .build_client(seed, birthday, true, self.regtest_network)
                .await;
            self.lightclients.push(lightclient);
            self
        }
        /// Stage blocks up to target height and update tree state.
        /// Does not apply block.
        pub async fn stage_blocks(
            &mut self,
            target_blockheight: u64,
            nonce: u64,
        ) -> &mut DarksideEnvironment {
            let count = target_blockheight - u64::from(self.staged_blockheight);
            self.darkside_connector
                .stage_blocks_create(
                    u32::from(self.staged_blockheight) as i32 + 1,
                    count as i32,
                    nonce as i32,
                )
                .await
                .unwrap();
            self.darkside_connector
                .add_tree_state(TreeState {
                    height: target_blockheight,
                    ..self.tree_state.clone()
                })
                .await
                .unwrap();
            self.staged_blockheight = BlockHeight::from(target_blockheight as u32);
            self
        }
        /// Apply blocks up to target height.
        pub async fn apply_blocks(&mut self, target_blockheight: u64) -> &mut DarksideEnvironment {
            self.darkside_connector
                .apply_staged(target_blockheight as i32)
                .await
                .unwrap();
            self
        }
        /// Stage and apply blocks up to target height and update tree state.
        pub async fn stage_and_apply_blocks(
            &mut self,
            target_blockheight: u64,
            nonce: u64,
        ) -> &mut DarksideEnvironment {
            self.stage_blocks(target_blockheight, nonce).await;
            self.apply_blocks(target_blockheight).await;
            self
        }
        /// Tool for chainbuilds.
        /// Stage a block and a send from funded lightclient, then write hex transaction to file.
        /// All sends in a chainbuild are appended to same file in order.
        /// Does not apply block.
        pub async fn send_and_write_transaction(
            &mut self,
            // We can't just take a reference to a LightClient, as that might be a reference to
            // a field of the DarksideScenario which we're taking by exclusive (i.e. mut) reference
            sender: DarksideSender<'_>,
            receiver_address: &str,
            value: u64,
            chainbuild_file: &File,
        ) -> &mut DarksideEnvironment {
            let (_, raw_tx) = self.send_transaction(sender, receiver_address, value).await;
            write_raw_transaction(&raw_tx, BranchId::Nu5, chainbuild_file);
            self
        }

        pub async fn send_transaction(
            &mut self,
            // We can't just take a reference to a LightClient, as that might be a reference to
            // a field of the DarksideScenario which we're taking by exclusive (i.e. mut) reference
            sender: DarksideSender<'_>,
            receiver_address: &str,
            value: u64,
        ) -> (&mut DarksideEnvironment, RawTransaction) {
            self.staged_blockheight = self.staged_blockheight.add(1);
            self.darkside_connector
                .stage_blocks_create(u32::from(self.staged_blockheight) as i32, 1, 0)
                .await
                .unwrap();
            let lightclient = match sender {
                DarksideSender::Faucet => self.get_faucet(),
                DarksideSender::IndexedClient(n) => self.get_lightclient(n),
                DarksideSender::ExternalClient(lc) => lc,
            };
            zingolib::testutils::lightclient::from_inputs::quick_send(
                lightclient,
                vec![(receiver_address, value, None)],
            )
            .await
            .unwrap();
            let mut streamed_raw_txns = self
                .darkside_connector
                .get_incoming_transactions()
                .await
                .unwrap();
            self.darkside_connector
                .clear_incoming_transactions()
                .await
                .unwrap();
            let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
            // There should only be one transaction incoming
            assert!(streamed_raw_txns.message().await.unwrap().is_none());
            self.darkside_connector
                .stage_transactions_stream(vec![(
                    raw_tx.data.clone(),
                    u64::from(self.staged_blockheight),
                )])
                .await
                .unwrap();
            self.tree_state = update_tree_states_for_transaction(
                &self.darkside_connector.0,
                raw_tx.clone(),
                u64::from(self.staged_blockheight),
            )
            .await;
            (self, raw_tx)
        }

        pub async fn shield_transaction(
            &mut self,
            // We can't just take a reference to a LightClient, as that might be a reference to
            // a field of the DarksideScenario which we're taking by exclusive (i.e. mut) reference
            sender: DarksideSender<'_>,
        ) -> (&mut DarksideEnvironment, RawTransaction) {
            self.staged_blockheight = self.staged_blockheight.add(1);
            self.darkside_connector
                .stage_blocks_create(u32::from(self.staged_blockheight) as i32, 1, 0)
                .await
                .unwrap();
            let lightclient = match sender {
                DarksideSender::Faucet => self.get_faucet(),
                DarksideSender::IndexedClient(n) => self.get_lightclient(n),
                DarksideSender::ExternalClient(lc) => lc,
            };
            // upgrade sapling
            lightclient.quick_shield().await.unwrap();
            let mut streamed_raw_txns = self
                .darkside_connector
                .get_incoming_transactions()
                .await
                .unwrap();
            self.darkside_connector
                .clear_incoming_transactions()
                .await
                .unwrap();
            let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
            // There should only be one transaction incoming
            assert!(streamed_raw_txns.message().await.unwrap().is_none());
            self.darkside_connector
                .stage_transactions_stream(vec![(
                    raw_tx.data.clone(),
                    u64::from(self.staged_blockheight),
                )])
                .await
                .unwrap();
            self.tree_state = update_tree_states_for_transaction(
                &self.darkside_connector.0,
                raw_tx.clone(),
                u64::from(self.staged_blockheight),
            )
            .await;
            (self, raw_tx)
        }

        /// Tool for chainbuilds.
        /// Stage a block and a shield from funded lightclient, then write hex transaction to file.
        /// Only one pool can be shielded at a time.
        /// All sends in a chainbuild are appended to same file in order.
        /// Does not apply block.
        pub async fn shield_and_write_transaction(
            &mut self,
            // We can't just take a reference to a LightClient, as that might be a reference to
            // a field of the DarksideScenario which we're taking by exclusive (i.e. mut) reference
            sender: DarksideSender<'_>,
            chainbuild_file: &File,
        ) -> &mut DarksideEnvironment {
            let (_, raw_tx) = self.shield_transaction(sender).await;
            write_raw_transaction(&raw_tx, BranchId::Nu5, chainbuild_file);
            self
        }
        /// Stage a block and transaction, then update tree state.
        /// Does not apply block.
        pub async fn stage_transaction(
            &mut self,
            hex_transaction: &str,
        ) -> &mut DarksideEnvironment {
            self.staged_blockheight = self.staged_blockheight.add(1);
            self.darkside_connector
                .stage_blocks_create(u32::from(self.staged_blockheight) as i32, 1, 0)
                .await
                .unwrap();
            self.darkside_connector
                .stage_transactions_stream(vec![(
                    hex::decode(hex_transaction).unwrap(),
                    u64::from(self.staged_blockheight),
                )])
                .await
                .unwrap();
            self.tree_state = update_tree_states_for_transaction(
                &self.darkside_connector.0,
                RawTransaction {
                    data: hex::decode(hex_transaction).unwrap(),
                    height: u64::from(self.staged_blockheight),
                },
                u64::from(self.staged_blockheight),
            )
            .await;
            self.darkside_connector
                .add_tree_state(self.tree_state.clone())
                .await
                .unwrap();
            self
        }
        /// Stage a block and next transaction in transaction set, then update tree state.
        /// Does not apply block.
        /// Temporary until tree states are also written to file.
        pub async fn stage_next_transaction(
            &mut self,
            transaction_set: &[String],
        ) -> &mut DarksideEnvironment {
            self.stage_transaction(&transaction_set[self.transaction_set_index as usize])
                .await;
            self.transaction_set_index += 1;
            self
        }

        /// Update the height of the staged blockchain
        pub fn set_staged_blockheight(&mut self, height: u64) {
            self.staged_blockheight = BlockHeight::from(height as u32);
        }
        /// Update the latest tree state
        pub fn set_tree_state(&mut self, tree_state: TreeState) {
            self.tree_state = tree_state;
        }

        pub fn get_handler(&self) -> &DarksideHandler {
            &self.darkside_handler
        }
        pub fn get_connector(&self) -> &DarksideConnector {
            &self.darkside_connector
        }
        pub fn get_client_builder(&self) -> &ClientBuilder {
            &self.client_builder
        }
        pub fn get_regtest_network(&self) -> &RegtestNetwork {
            &self.regtest_network
        }
        pub fn get_faucet(&self) -> &LightClient {
            self.faucet
                .as_ref()
                .expect("scenario should have a faucet lightclient")
        }
        pub fn get_lightclient(&self, lightclient_index: u64) -> &LightClient {
            &self.lightclients[lightclient_index as usize]
        }
        pub fn get_staged_blockheight(&self) -> &BlockHeight {
            &self.staged_blockheight
        }
        pub fn get_tree_state(&self) -> &TreeState {
            &self.tree_state
        }
    }

    /// A way to specify which client to send funds from
    pub enum DarksideSender<'a> {
        // The faucet of the DarksideScenario
        Faucet,
        // A generated non-faucet client, accessed by index
        IndexedClient(u64),
        // A client not managed by the DarksideScenario itself
        ExternalClient(&'a LightClient),
    }
}
