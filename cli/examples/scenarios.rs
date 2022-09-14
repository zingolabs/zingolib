use zingoconfig::ZingoConfig;

fn main() {
    #[allow(unused_variables)]
    let rm = zingo_cli::regtest::RegtestManager::new().launch(true);
    #[allow(unused_variables)]
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
}
#[test]
fn prove_scenario_is_built() {}
