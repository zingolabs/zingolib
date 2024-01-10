fn main() {
    start_iplink_onoff(2, 10);
    zingo_cli::run_cli();
}

fn start_iplink_onoff(sleeptime: u8, uptime: u8) {
    std::process::Command::new("watch")
        .args([
            "-n",
            &(sleeptime + uptime).to_string(),
            "./network_updown.sh",
            &sleeptime.to_string(),
        ])
        .spawn()
        .unwrap();
}
