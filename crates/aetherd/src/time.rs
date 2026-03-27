use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

pub(crate) fn current_unix_timestamp_secs() -> i64 {
    unix_timestamp_secs(SystemTime::now().duration_since(UNIX_EPOCH))
}

pub(crate) fn current_unix_timestamp_millis() -> i64 {
    unix_timestamp_millis(SystemTime::now().duration_since(UNIX_EPOCH))
}

fn unix_timestamp_secs(duration_since_epoch: Result<Duration, SystemTimeError>) -> i64 {
    match duration_since_epoch {
        Ok(duration) => duration.as_secs() as i64,
        Err(err) => {
            tracing::warn!(error = %err, "system clock before Unix epoch, using 0");
            0
        }
    }
}

fn unix_timestamp_millis(duration_since_epoch: Result<Duration, SystemTimeError>) -> i64 {
    match duration_since_epoch {
        Ok(duration) => duration.as_millis() as i64,
        Err(err) => {
            tracing::warn!(error = %err, "system clock before Unix epoch, using 0");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use std::time::UNIX_EPOCH;

    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::fmt::MakeWriter;

    use super::{unix_timestamp_millis, unix_timestamp_secs};

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(self.0.clone())
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
        let buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(buffer.clone())
            .finish();
        let result = dispatcher::with_default(&Dispatch::new(subscriber), run);
        let logs = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("utf8 logs");
        (result, logs)
    }

    #[test]
    fn unix_timestamp_helpers_log_on_clock_error() {
        let secs_error = UNIX_EPOCH
            .duration_since(std::time::SystemTime::now())
            .expect_err("clock error");
        let millis_error = UNIX_EPOCH
            .duration_since(std::time::SystemTime::now())
            .expect_err("clock error");

        let (seconds, sec_logs) = capture_logs(|| unix_timestamp_secs(Err(secs_error)));
        let (millis, ms_logs) = capture_logs(|| unix_timestamp_millis(Err(millis_error)));

        assert_eq!(seconds, 0);
        assert_eq!(millis, 0);
        assert!(sec_logs.contains("system clock before Unix epoch, using 0"));
        assert!(ms_logs.contains("system clock before Unix epoch, using 0"));
    }
}
