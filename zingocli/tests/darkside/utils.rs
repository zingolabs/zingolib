use std::{
    path::PathBuf,
    process::{Child, Command},
};

use zingo_cli::regtest::get_regtest_dir;

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
        let log_file = &darkside_dir.join("lwd_log").to_string_lossy().to_string();
        let grpc_bind_addr = format!("127.0.0.1:{grpc_port}");
        let lightwalletd_handle = Command::new(get_regtest_dir().join("bin").join("lightwalletd"))
            .args([
                "--darkside-very-insecure",
                "--no-tls-very-insecure",
                "--data-dir",
                &darkside_dir.to_string_lossy().to_string(),
                "--log-file",
                log_file,
                "--grpc-bind-addr",
                &grpc_bind_addr,
            ])
            .spawn()
            .unwrap();

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
