//! The writer of the opt-in host-side timing record (`BUTTERPOLLO_TIMING_RECORD=<dir>`). Rows
//! and the file text come from `butterpollo_core::timing_record`; this is the file IO on a
//! thread of its own, so the send path only does a non-blocking queue push. Unset, no thread
//! and no file exist.
use anyhow::{Context, Result};
use butterpollo_core::timing_record::{self as record, Meta, Row, Sink};
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

/// How long rows may wait in the file buffer when none follow.
const FLUSH_AFTER: Duration = Duration::from_millis(250);

/// The record directory the environment asks for, if any.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os(record::ENV)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// `<dir>/host-<stream_id>.csv`, with anything but letters, digits, `-` and `_` in the id
/// replaced.
pub fn path(dir: &Path, stream_id: &str) -> PathBuf {
    let id: String = stream_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("host-{id}.csv"))
}

/// Creates the stream's file and starts its writer. The writer ends, writing the footer,
/// once the returned sink and its clones are dropped.
pub fn start(dir: &Path, stream_id: &str, meta: Meta) -> Result<Arc<Sink>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating the timing record directory {}", dir.display()))?;
    let file_path = path(dir, stream_id);
    let file = std::fs::File::create(&file_path)
        .with_context(|| format!("creating the timing record {}", file_path.display()))?;
    spawn(file, meta, record::QUEUE_ROWS, &file_path)
}

/// The stream's sink when `dir` is set, else `None` without touching the file system or
/// starting a thread. A directory that cannot be used is reported and the stream goes on
/// without a record.
pub fn attach(dir: Option<&Path>, stream_id: &str, meta: Meta) -> Option<Arc<Sink>> {
    let dir = dir?;
    match start(dir, stream_id, meta) {
        Ok(sink) => {
            tracing::info!(dir = %dir.display(), stream_id, "writing the host timing record");
            Some(sink)
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "host timing record is off");
            None
        }
    }
}

fn spawn(
    out: impl Write + Send + 'static,
    meta: Meta,
    capacity: usize,
    name: &Path,
) -> Result<Arc<Sink>> {
    let (sink, rows) = record::channel(capacity);
    let dropped = sink.dropped_counter();
    let name = name.display().to_string();
    thread::Builder::new()
        .name("timing-record".into())
        .spawn(move || {
            if let Err(error) = write(out, &meta, rows, &dropped) {
                tracing::warn!(%error, file = %name, "timing record writer stopped");
            }
        })
        .context("starting the timing record writer")?;
    Ok(Arc::new(sink))
}

/// Writes the header, then rows as they arrive, then the footer once the senders are gone.
fn write(
    out: impl Write,
    meta: &Meta,
    rows: Receiver<Row>,
    dropped: &AtomicU64,
) -> std::io::Result<()> {
    let mut out = BufWriter::with_capacity(4096, out);
    out.write_all(record::header(meta).as_bytes())?;
    loop {
        match rows.recv_timeout(FLUSH_AFTER) {
            Ok(row) => out.write_all(row.line().as_bytes())?,
            Err(RecvTimeoutError::Timeout) => out.flush()?,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    out.write_all(record::footer(dropped.load(Ordering::Relaxed)).as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::performance::Stamps;
    use std::{
        sync::{Mutex, mpsc},
        time::Instant,
    };

    fn meta() -> Meta {
        Meta {
            codec: "h264",
            capture: "dda".into(),
            present_source: "capture",
            media_epoch_qpc_ms: 1000.5,
        }
    }

    fn stamps(epoch: Instant, base: u64) -> Stamps {
        let us = |n: u64| epoch + Duration::from_micros(base + n);
        Stamps {
            presented: us(0),
            acquired: us(150),
            published: us(200),
            claimed: us(260),
            submitted: us(500),
            encoded: us(2_900),
            packetized: us(3_000),
            prepared: us(3_100),
            first_sent: us(3_150),
            last_sent: us(3_400),
        }
    }

    /// A destination the test can read after the writer is done.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn rows_in_the_file_equal_the_stamp_points() {
        let epoch = Instant::now();
        let out = Shared::default();
        let (sink, rows) = record::channel(16);
        let dropped = sink.dropped_counter();
        for (frame, base) in [(10_u32, 1_000_u64), (11, 17_667)] {
            sink.record(Row::new(
                frame,
                (base * 90 / 1000) as u32,
                &stamps(epoch, base),
                epoch,
            ));
        }
        drop(sink);
        write(out.clone(), &meta(), rows, &dropped).unwrap();
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "# rubylight timing record 1");
        assert_eq!(lines[1], "# media_epoch_qpc_ms=1000.500");
        assert_eq!(lines[2], "# codec=h264 capture=dda present_source=capture");
        assert_eq!(lines[3], record::COLUMNS);
        assert_eq!(
            lines[4],
            "10,1000,1000,1150,1200,1260,1500,3900,4000,4100,4150,4400"
        );
        assert_eq!(
            lines[5],
            "11,17666,17667,17817,17867,17927,18167,20567,20667,20767,20817,21067"
        );
        assert_eq!(lines[6], "# dropped=0");
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn a_stalled_writer_never_blocks_the_send_path_and_the_drops_are_counted() {
        /// Blocks every write until the test lets it go.
        struct Stalled {
            go: Receiver<()>,
            out: Shared,
        }
        impl Write for Stalled {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                let _ = self.go.recv();
                self.out.write(data)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let epoch = Instant::now();
        let out = Shared::default();
        let (release, go) = mpsc::channel();
        let sink = spawn(
            Stalled {
                go,
                out: out.clone(),
            },
            meta(),
            8,
            Path::new("stalled"),
        )
        .unwrap();
        let row = Row::new(1, 0, &stamps(epoch, 0), epoch);
        let started = Instant::now();
        // The buffer holds about 45 rows and the queue 8, so most of these have nowhere to go.
        for _ in 0..1_000 {
            sink.record(row);
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the send path waited for the writer: {:?}",
            started.elapsed()
        );
        let dropped = sink.dropped();
        assert!(dropped >= 900, "dropped {dropped}");
        // Let the writer go: it ends with the footer naming the same count.
        drop(release);
        drop(sink);
        for _ in 0..200 {
            if out
                .0
                .lock()
                .unwrap()
                .ends_with(format!("# dropped={dropped}\n").as_bytes())
            {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("no footer with dropped={dropped}");
    }

    #[test]
    fn nothing_is_created_unless_the_record_is_on() {
        let dir = std::env::temp_dir().join(format!("timing-record-{}", uuid::Uuid::new_v4()));
        assert!(attach(None, "7", meta()).is_none());
        assert!(!dir.exists());
        assert_eq!(
            path(Path::new("rec"), "a/b c"),
            Path::new("rec").join("host-a_b_c.csv")
        );
        // Switched on, the stream's file appears with its header, and the footer follows
        // once the sink is dropped.
        let sink = attach(Some(&dir), "7", meta()).unwrap();
        let file = dir.join("host-7.csv");
        assert!(file.exists());
        drop(sink);
        for _ in 0..200 {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            if text.ends_with(
                "# dropped=0
",
            ) {
                assert!(text.starts_with(
                    "# rubylight timing record 1
"
                ));
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("the file was not finished");
    }
}
