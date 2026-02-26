pub mod scenarios;

pub(crate) fn localhost_uri(port: u16) -> http::Uri {
    format!("http://127.0.0.1:{port}").parse().unwrap()
}
