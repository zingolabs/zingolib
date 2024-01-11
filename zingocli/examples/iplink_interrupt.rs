use std::{
    thread::{self, sleep},
    time::Duration,
};

fn main() {
    start_iplink_onoff(2, 10);
    zingo_cli::run_cli();
}

fn start_iplink_onoff(sleeptime: u64, uptime: u64) {
    let _ = thread::spawn(move || loop {
        std::process::Command::new("ip")
            .args(["link", "set", "wlp1s0", "down"])
            .output()
            .unwrap();
        sleep(Duration::from_secs(sleeptime));
        std::process::Command::new("ip")
            .args(["link", "set", "wlp1s0", "up"])
            .output()
            .unwrap();
        sleep(Duration::from_secs(uptime));
    });
}
