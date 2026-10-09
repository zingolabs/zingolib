use std::io::Write;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const QUIT_GRACE: Duration = Duration::from_secs(15);

const POLL: Duration = Duration::from_millis(100);

/// - writes `quit` to the child's stdin
/// - sends SIGKILL to the child's process group once the grace period ends
/// - reaps the child
pub fn quit(child: &mut Child) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "quit");
        let _ = stdin.flush();
    }
    let deadline = Instant::now() + QUIT_GRACE;
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(POLL);
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .status();
    let _ = child.wait();
}
