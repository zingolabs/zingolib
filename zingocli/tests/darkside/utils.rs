use std::{
    path::PathBuf,
    process::{Child, Command},
    time::Duration,
};

use zingo_cli::regtest::{get_regtest_dir, launch_lightwalletd};

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
