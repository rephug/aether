//! The only test in its binary, so it can set the process environment before SurrealDB
//! reads it at the first open.

use std::io::Write;
use std::sync::{Arc, Mutex};

use aether_store::{SurrealGraphStore, set_surrealkv_sizing_env};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn first_open_uses_the_fixed_surrealkv_sizes() {
    // SAFETY: this binary runs no other test, and nothing else touches the environment.
    unsafe { set_surrealkv_sizing_env() };
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let temp = tempfile::tempdir().expect("tempdir");
    SurrealGraphStore::open(temp.path())
        .await
        .expect("open graph store");

    // SurrealDB logs the sizes it uses; these are 512 MiB and 256 MiB, not a size computed
    // from the machine's memory.
    let logs = String::from_utf8(logs.0.lock().expect("log buffer").clone()).expect("utf8");
    assert!(
        logs.contains("Setting block cache capacity: 536870912"),
        "{logs}"
    );
    assert!(
        logs.contains("Setting value log max file size: 268435456"),
        "{logs}"
    );
}
