use std::{io, sync};

use tracing_subscriber::fmt;

static INIT: sync::Once = sync::Once::new();

/// Routes this crate's `log` records and nym-sdk's `tracing` events to logcat under the `MixnetProxy` tag.
pub(crate) fn init() {
    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Debug)
                .with_tag("MixnetProxy"),
        );
        let _ = fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(Logcat)
            .try_init();
    });
}

/// Hands the `tracing` formatter a fresh line sink for every event.
struct Logcat;

/// Forwards one formatted `tracing` event to the `log` facade, and so to logcat.
struct LogcatLine;

impl io::Write for LogcatLine {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        log::info!("{}", String::from_utf8_lossy(buf).trim_end());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> fmt::MakeWriter<'a> for Logcat {
    type Writer = LogcatLine;

    fn make_writer(&'a self) -> Self::Writer {
        LogcatLine
    }
}
