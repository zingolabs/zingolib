use crate::utils::scenarios::setup::ClientManager;
use json::JsonValue;
use rand::Rng;
use tempdir::{self, TempDir};

#[tokio::test]
async fn test_simple_sync() {
    let mut rng = rand::thread_rng();

    let num: u32 = rng.gen_range(0..100000);
    let temp_dir = TempDir::new(&format!("dwld_test_{num}")).unwrap();
    let path = temp_dir.path().to_path_buf();

    let server_id = zingoconfig::construct_server_uri(Some(format!("http://127.0.0.1:9067")));

    let client = ClientManager::new(
            server_id,
            path,
            "still champion voice habit trend flight survey between bitter process artefact blind carbon truly provide dizzy crush flush breeze blouse charge solid fish spread"
        )
            .build_new_faucet(663150, true).await;

    let result = client.do_sync(true).await.unwrap();

    println!("{}", result);
    assert!(result.has_key("result"));
    let res_value = match result {
        JsonValue::Object(res) => res,
        _ => panic!("Expected and object got something else"),
    };

    assert_eq!(res_value["result"], "success");
    assert_eq!(res_value["latest_block"], 663200);
    assert_eq!(res_value["total_blocks_synced"], 50);
}
