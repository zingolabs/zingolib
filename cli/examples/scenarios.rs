use zingo_cli::regtest::generate_n_blocks;
use zingoconfig::ZingoConfig;
use zingolib::{create_on_data_dir, lightclient::LightClient};

fn main() {
    #[allow(unused_variables)]
    let rm = zingo_cli::regtest::RegtestManager::new().launch(true);
    #[allow(unused_variables)]
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
}
#[test]
fn prove_scenario_is_built() {}

fn send_sapling_to_self() -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Add regtestmanager test-local directories,
    // and all other bits and bobs needed to run multiple zcashds
    // so that multiple scenarios can run concurrently
    //let (manager, zcashd_handle, lightwalletd_handle) =
    //    zingo_cli::regtest::RegtestManager::launch(true);
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_on_data_dir(server, None)?;
    let sapling_key = std::fs::read_to_string("privkey")?;
    let client = LightClient::new_from_phrase(sapling_key, &config, 0, false)?;
    // Doesn't compile, private fields
    //generate_n_blocks(&manager.bin_location, &manager.zcashd_config, 5);
    Ok(())
}
