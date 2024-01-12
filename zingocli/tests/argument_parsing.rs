#[test]
fn clargs_view_key_birthday_fresh_wallet_dir() {
    // Remove the 'foo' directory if it exists
    let cli_parse_test_data = "./cli_parse_test_data";
    let cpdp = std::path::Path::new(cli_parse_test_data);
    if cpdp.exists() {
        std::fs::remove_dir_all(cli_parse_test_data)
            .expect("Failed to remove existing foo directory");
    }

    // Run the cargo command
    let output = std::process::Command::new("cargo")
        .args(&["run", "--"])
        .args(&["--view-key", zingolib::testvectors::MAINNET_ALPHA_VIEWKEY]) // shortened for brevity
        .args(&["--birthday", "2363649"])
        .args(&["--fresh-wallet-dir", cli_parse_test_data])
        .args(&["help"])
        .output()
        .expect("Failed to execute cargo run command");

    // Check that the command executed successfully
    assert!(output.status.success());

    // Additional assertions based on the expected behavior of your application
    // For example, check if the 'foo' directory was created
    assert!(std::fs::metadata(cpdp).is_ok());
    if std::fs::metadata(cli_parse_test_data).is_ok() {
        std::fs::remove_dir_all(cli_parse_test_data)
            .expect("Failed to remove existing foo directory");
    }
}
