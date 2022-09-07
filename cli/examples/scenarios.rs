use zingoconfig::ZingoConfig;

fn main() {
    zingo_cli::regtest::launch(true);
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
}
#[test]
fn prove_scenario_is_built() {}
