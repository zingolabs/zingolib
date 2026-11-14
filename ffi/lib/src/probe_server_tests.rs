use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

use zingolib::netutils::time::INDEXER_CONNECT_TIMEOUT;

use super::*;

const LOOPBACK_ANY_PORT: &str = "127.0.0.1:0";
const BACKLOG_FULL_SIGNAL: Duration = Duration::from_millis(250);
const TIMER_SLACK: Duration = Duration::from_secs(5);
const H2_PREFACE_LEN: usize = 24;
const H2_FRAME_HEADER_LEN: usize = 9;
const H2_HEADERS_FRAME: u8 = 0x1;
const READ_CHUNK: usize = 4096;

fn outcome(server: &str) -> json::JsonValue {
    json::parse(&probe_server(server.to_string()).expect("a probe reports"))
        .expect("a probe reports json")
}

fn timed_outcome(server: &str) -> (json::JsonValue, Duration) {
    let started = Instant::now();
    let report = outcome(server);
    (report, started.elapsed())
}

fn local_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind(LOOPBACK_ANY_PORT).expect("a local listener");
    let port = listener.local_addr().expect("a local address").port();
    (listener, port)
}

fn free_port() -> u16 {
    local_listener().1
}

fn serve(handle: fn(TcpStream)) -> u16 {
    let (listener, port) = local_listener();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream);
        }
    });
    port
}

fn blackhole() -> (TcpListener, Vec<TcpStream>, u16) {
    let (listener, port) = local_listener();
    let address = listener.local_addr().expect("a local address");
    let mut queued = Vec::new();
    while let Ok(stream) = TcpStream::connect_timeout(&address, BACKLOG_FULL_SIGNAL) {
        queued.push(stream);
    }
    (listener, queued, port)
}

fn read_until_headers_frame(stream: &mut TcpStream) {
    let mut received = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = stream.read(&mut chunk).expect("the client keeps writing");
        assert!(read > 0, "the client closed before sending a request");
        received.extend_from_slice(&chunk[..read]);
        let mut cursor = H2_PREFACE_LEN;
        while received.len() >= cursor + H2_FRAME_HEADER_LEN {
            if received[cursor + 3] == H2_HEADERS_FRAME {
                return;
            }
            let length = usize::from(received[cursor]) << 16
                | usize::from(received[cursor + 1]) << 8
                | usize::from(received[cursor + 2]);
            cursor += H2_FRAME_HEADER_LEN + length;
        }
    }
}

fn close_after_the_request(mut stream: TcpStream) {
    read_until_headers_frame(&mut stream);
}

fn never_answer(mut stream: TcpStream) {
    let mut sink = [0u8; READ_CHUNK];
    while matches!(stream.read(&mut sink), Ok(read) if read > 0) {}
}

fn assert_no_answer_after(report: &json::JsonValue, elapsed: Duration, bound: Duration) {
    assert_eq!(report["outcome"], "noAnswer", "{report:#}");
    assert_eq!(report["after_seconds"], bound.as_secs());
    assert!(
        elapsed >= bound,
        "gave up after {elapsed:?}, before {bound:?}"
    );
    assert!(
        elapsed < bound + TIMER_SLACK,
        "gave up after {elapsed:?}, long past {bound:?}"
    );
}

#[test]
fn an_address_without_a_host_is_invalid_input() {
    for server in ["", "http://"] {
        assert!(
            matches!(
                probe_server(server.to_string()),
                Err(ZingolibError::InvalidInput(_))
            ),
            "{server:?} has no host"
        );
    }
}

#[test]
fn a_host_that_does_not_resolve_reports_unresolved() {
    let report = outcome("https://zingo-probe.invalid:9067");
    assert_eq!(report["outcome"], "unresolved");
    assert_eq!(report["host"], "zingo-probe.invalid");
    assert_eq!(report["port"], 9067);
}

#[test]
fn a_closed_port_on_a_literal_address_reports_unreachable() {
    let report = outcome(&format!("http://127.0.0.1:{}", free_port()));
    assert_eq!(report["outcome"], "unreachable");
    assert_eq!(report["literal"], true);
    assert_eq!(report["resolved"][0], "127.0.0.1");
}

#[test]
fn a_closed_port_on_a_resolved_name_reports_unreachable_with_its_addresses() {
    let report = outcome(&format!("http://localhost:{}", free_port()));
    assert_eq!(report["outcome"], "unreachable", "{report:#}");
    assert_eq!(report["literal"], false);
    assert!(
        report["resolved"]
            .members()
            .any(|address| address == "127.0.0.1" || address == "::1"),
        "{report:#}"
    );
}

#[test]
fn a_target_that_drops_every_connection_reports_no_answer_after_the_connect_bound() {
    let (_listener, _queued, port) = blackhole();
    let (report, elapsed) = timed_outcome(&format!("http://127.0.0.1:{port}"));
    assert_no_answer_after(&report, elapsed, INDEXER_CONNECT_TIMEOUT);
}

#[test]
fn a_server_that_never_answers_reports_no_answer_after_the_request_bound() {
    let port = serve(never_answer);
    let (report, elapsed) = timed_outcome(&format!("http://127.0.0.1:{port}"));
    assert_no_answer_after(&report, elapsed, PROBE_REQUEST_TIMEOUT);
}

#[test]
fn a_connection_the_server_closes_mid_request_is_unreachable_not_refused() {
    let port = serve(close_after_the_request);
    let report = outcome(&format!("http://127.0.0.1:{port}"));
    assert_eq!(report["outcome"], "unreachable", "{report:#}");
}

#[test]
fn a_verified_report_carries_the_server_info_as_fields() {
    let info = data::ServerInfo {
        version: "0.1.0".to_string(),
        git_commit: String::new(),
        server_uri: "http://127.0.0.1:9067".parse().expect("a server uri"),
        vendor: "zingo-test".to_string(),
        taddr_support: true,
        chain_name: "regtest".to_string(),
        sapling_activation_height: 1,
        consensus_branch_id: String::new(),
        latest_block_height: 7,
    };
    let probe = Probe::Verified {
        target: ProbeTarget {
            host: "127.0.0.1".to_string(),
            port: 9067,
            resolved: vec![std::net::Ipv4Addr::LOCALHOST.into()],
            literal: true,
        },
        latency_ms: 1,
        info: Box::new(info),
    };
    let report = json::parse(&probe.report().expect("a probe reports")).expect("json");
    assert_eq!(report["details"]["vendor"], "zingo-test", "{report:#}");
    assert_eq!(report["details"]["version"], "0.1.0");
    assert_eq!(report["details"]["chain_name"], "regtest");
    assert_eq!(report["details"]["latest_block_height"], 7);
}
