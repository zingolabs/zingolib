pub mod scenarios;

pub fn localhost_uri(port: u16) -> http::Uri {
    format!("http://127.0.0.1:{port}").parse().unwrap()
}
