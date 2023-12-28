use zingo_testutils::scenarios::setup::ClientBuilder;
use zingolib::lightclient::LightClient;

#[tokio::test]
async fn equality() {
    let tmpdir = tempdir::TempDir::new("zingo_live_test")
        .unwrap()
        .into_path();
    let client_build = ClientBuilder::new(x, tmpdir);
}
