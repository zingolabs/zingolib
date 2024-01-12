#[test]
fn clargs_view_key_birthday_fresh_wallet_dir() {
    let expected_output: &str = r#"{
  "ufvk": "uview1ah2qw247djujxwu5tdr20t7naaqvd5qkljxtm0yrw92tzy6fvafdhv7uchvsrzxaqskc7mxphzafgn5aca6pscrdx8xqu95ruyefng0hcctsnle4jq9f97gvymlf78pal7zqwf3yfej6han20pvhj0j0ew05dzq685kx29phyy5ffuw63wxmvesm9r23jhccfrdm9yxr5tz4hkw40t0ak5j4lgr67zdcl9rhluaqwatudjhaq0lep5ffcc8yrruvf0tz9zmxl5zfg9lx728mwdz4705wsr9fr8h4u7nc5ja8c560j45jn0jqty8hcqjedjakjkj04najmvzk4fr0g9kevshe6verg8h9pr4phx86wsc2xa5rdp78zrr5upuyqanhk98w4u3vs9mssdnxrwe9xf4qlffqq46faaxuvnsst4mn87eyk4j6h4jm6t3kzcmwh0waw8u5500yqyfm35ndcerzkx8xr5xaznqrma0zy69qvgz7nnyq",
  "birthday": 2363649
}
"#;
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
        .args(&["--nosync"])
        .args(&["exportufvk"])
        .output()
        .expect("Failed to execute cargo run command");

    // Check that the command executed successfully
    assert!(output.status.success());

    // Additional assertions based on the expected behavior of your application
    // For example, check if the 'foo' directory was created
    assert!(std::fs::metadata(cpdp).is_ok());
    assert_eq!(
        std::string::String::from_utf8_lossy(&output.stdout),
        expected_output
    );
    if std::fs::metadata(cli_parse_test_data).is_ok() {
        std::fs::remove_dir_all(cli_parse_test_data)
            .expect("Failed to remove existing foo directory");
    }
}
