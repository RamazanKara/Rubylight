//! The support bundle (`/api/logs/export_crash`): the host's logs and diagnostics with what
//! identifies the person redacted ([`butterpollo_session::log_redact`]), and the newest crash dump
//! only when the console asks for it (`include_dump`): a memory dump holds whatever the host had in
//! memory, which no redaction can reach.

use super::{Dump, log_path, newest_dump};
use crate::state::{Host, Shared};
use anyhow::{Context, Result};
use butterpollo_session::log_redact::Redactor;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// The most of each log the bundle keeps: its end.
const LOG_TAIL: u64 = 8 * 1024 * 1024;
/// A larger dump is left out.
const MAX_DUMP: u64 = 512 * 1024 * 1024;
/// Settings whose values the bundle never shows.
const SECRET_SETTINGS: &[&str] = &[
    "password", "token", "key", "secret", "command", "cmd", "webhook",
];

/// The bundle's parts and their estimated size; the dump counts only when it is included.
pub fn manifest(h: &Shared, include_dump: bool) -> Value {
    let dump = if include_dump {
        newest_dump(h).map_or(0, |d| d.size)
    } else {
        0
    };
    json!({"status":true,"include_dump":include_dump,"parts":[{"index":1,"filename":"butterpollo-support.zip","estimated_size_bytes":dump+LOG_TAIL}]})
}

/// Writes the bundle to the host's `support` folder and returns its path.
pub fn bundle(h: &Shared, include_dump: bool) -> Result<PathBuf> {
    bundle_with_dump(h, || include_dump.then(|| newest_dump(h)).flatten())
}

/// Every name of a user, the PC, a paired device or an app the logs may hold.
fn redactor(h: &Host) -> Redactor {
    let mut redactor = Redactor::new();
    if let Some(name) = std::env::var_os("USERNAME") {
        redactor.user(&name.to_string_lossy());
    }
    if let Some(profile) = std::env::var_os("USERPROFILE")
        && let Some(name) = Path::new(&profile).file_name()
    {
        redactor.user(&name.to_string_lossy());
    }
    // The profile folders name every user of this PC, the signed-in one among them.
    let drive = std::env::var_os("SystemDrive").unwrap_or_else(|| "C:".into());
    let users = PathBuf::from(drive).join("\\Users");
    for entry in std::fs::read_dir(users).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            redactor.user(&entry.file_name().to_string_lossy());
        }
    }
    if let Some(name) = butterpollo_win_io::net::host_name() {
        redactor.pc(&name);
    }
    redactor.pc(&crate::network::host_name(&h.config.read().unwrap()));
    for client in &h.paired.read().unwrap().clients {
        redactor.device(&client.name);
    }
    for app in h.apps.read().unwrap().iter() {
        redactor.app(&app.name);
    }
    redactor
}

/// Copies the end of the log at `source`, line by line through `redactor`, into `writer`. A line
/// the cut fell into is left out.
fn copy_redacted(source: &Path, writer: &mut impl Write, redactor: &Redactor) -> Result<bool> {
    let Ok(mut file) = std::fs::File::open(source) else {
        return Ok(false);
    };
    let length = file.metadata()?.len();
    let start = length.saturating_sub(LOG_TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(LOG_TAIL));
    let mut line = Vec::new();
    if start > 0 {
        reader.read_until(b'\n', &mut line)?;
    }
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(true);
        }
        writer.write_all(redactor.line(&String::from_utf8_lossy(&line)).as_bytes())?;
    }
}

pub(super) fn bundle_with_dump(h: &Shared, dump: impl FnOnce() -> Option<Dump>) -> Result<PathBuf> {
    use zip::{ZipWriter, write::SimpleFileOptions};
    let directory = h.directory.join("support");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.zip", uuid::Uuid::new_v4()));
    let mut writer = ZipWriter::new(std::fs::File::create(&path)?);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let redactor = redactor(h);
    let host_log = log_path(h);
    for (name, source) in [
        ("logs/butterpollo.log", host_log.clone()),
        (
            "logs/butterpollo.log.1",
            butterpollo_session::logfile::RotatingFile::rotated(&host_log, 1),
        ),
        ("logs/service.log", h.directory.join("logs/service.log")),
        ("crashes/panic.txt", h.directory.join("crashes/panic.txt")),
    ] {
        if source.is_file() {
            writer.start_file(name, options)?;
            copy_redacted(&source, &mut writer, &redactor)?;
        }
    }
    let dump = dump();
    writer.start_file("diagnostics.json", options)?;
    let config = h.config.read().unwrap();
    let values: serde_json::Map<_, _> = config
        .values
        .iter()
        .map(|(k, v)| {
            let value = if SECRET_SETTINGS.iter().any(|s| k.contains(s)) {
                "[redacted]".to_owned()
            } else {
                redactor.line(v)
            };
            (k.clone(), json!(value))
        })
        .collect();
    let diagnostics = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": "windows",
        "redacted": "IP and MAC addresses, user names and folders, the PC's and devices' names, game titles and secrets are replaced by marks such as [lan-ip], [ip], [mac], [user], [pc], [device-1], [app-1] and [redacted].",
        "crash_dump_included": dump.is_some(),
        "config": values,
        "codecs": h.codecs.load(std::sync::atomic::Ordering::Acquire),
    });
    writer.write_all(&serde_json::to_vec_pretty(&diagnostics)?)?;
    drop(config);
    if let Some(dump) = dump {
        let name = dump
            .path
            .file_name()
            .context("dump filename missing")?
            .to_string_lossy()
            .into_owned();
        writer.start_file(format!("crashes/{name}"), options)?;
        std::io::copy(
            &mut std::fs::File::open(dump.path)?.take(MAX_DUMP),
            &mut writer,
        )?;
    }
    writer.finish()?.sync_all()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    fn entries(path: &Path) -> Vec<(String, String)> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut file = archive.by_index(index).unwrap();
                let mut text = String::new();
                let _ = file.read_to_string(&mut text);
                (file.name().to_owned(), text)
            })
            .collect()
    }

    fn fixture() -> (Fixture, PathBuf) {
        let f = Fixture::new();
        f.client(u32::MAX);
        let mut app = butterpollo_session::state::App::desktop();
        app.name = "Secret Game Title".into();
        f.host.apps.write().unwrap().push(app);
        let logs = f.host.directory.join("logs");
        std::fs::create_dir(&logs).unwrap();
        std::fs::write(
            logs.join("butterpollo.log"),
            "2026-11-26T10:00:00.000000Z  INFO butterpollo::nvhttp: launch client=\"Parity device\" peer=192.168.4.20:50123 app=\"Secret Game Title\" mac=AA:BB:CC:DD:EE:FF wan=203.0.113.9 path=C:\\Users\\someone\\Games pin=4821 codec=hevc driver=32.0.12033.1030 elapsed_ms=12 code=\"video_encoder\"\n",
        )
        .unwrap();
        std::fs::write(logs.join("service.log"), "service log fixture\n").unwrap();
        *f.host.config.write().unwrap() = butterpollo_config::config::Config::parse("sunshine_name=Living Room PC\nexternal_ip=203.0.113.7\npassword=secret-password\nserver_cmd=[{\"cmd\":\"secret-command\"}]\napi_token=secret-token").unwrap();
        let dump = f.host.directory.join("crashes");
        std::fs::create_dir_all(&dump).unwrap();
        let dump = dump.join("butterpollo.exe.1234.dmp");
        std::fs::write(&dump, b"MDMP heap with secrets").unwrap();
        (f, dump)
    }

    fn the_dump(path: &Path) -> Dump {
        Dump {
            path: path.to_owned(),
            size: 22,
            modified: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn the_default_bundle_has_redacted_logs_and_no_memory_dump() {
        let (f, dump) = fixture();
        // A dump exists, but only an explicit request includes it.
        let path = bundle(&f.host, false).unwrap();
        let entries = entries(&path);
        let names: Vec<_> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "logs/butterpollo.log",
                "logs/service.log",
                "diagnostics.json"
            ]
        );
        assert!(!names.iter().any(|n| n.ends_with(".dmp")));
        let everything: String = entries.iter().map(|(_, text)| text.as_str()).collect();
        for leaked in [
            "Parity device",
            "192.168.4.20",
            "Secret Game Title",
            "AA:BB:CC:DD:EE:FF",
            "203.0.113.9",
            "203.0.113.7",
            "someone",
            "4821",
            "Living Room PC",
            "secret-password",
            "secret-command",
            "secret-token",
            "MDMP",
        ] {
            assert!(!everything.contains(leaked), "{leaked} leaked");
        }
        let log = &entries[0].1;
        for kept in [
            "INFO butterpollo::nvhttp: launch",
            "client=\"[device-1]\"",
            "peer=[lan-ip]:50123",
            "app=\"[app-1]\"",
            "codec=hevc driver=32.0.12033.1030 elapsed_ms=12 code=\"video_encoder\"",
        ] {
            assert!(log.contains(kept), "{kept} not in {log}");
        }
        let diagnostics: Value = serde_json::from_str(&entries[2].1).unwrap();
        assert_eq!(diagnostics["crash_dump_included"], false);
        assert_eq!(diagnostics["config"]["sunshine_name"], "[pc]");
        assert_eq!(diagnostics["config"]["server_cmd"], "[redacted]");
        assert_eq!(manifest(&f.host, false)["include_dump"], false);
        let _ = dump;
    }

    #[test]
    fn the_memory_dump_comes_only_when_asked_for() {
        let (f, dump) = fixture();
        let path = bundle_with_dump(&f.host, || Some(the_dump(&dump))).unwrap();
        let entries = entries(&path);
        assert!(
            entries
                .iter()
                .any(|(name, text)| name == "crashes/butterpollo.exe.1234.dmp"
                    && text.starts_with("MDMP"))
        );
        let diagnostics = &entries
            .iter()
            .find(|(n, _)| n == "diagnostics.json")
            .unwrap()
            .1;
        let diagnostics: Value = serde_json::from_str(diagnostics).unwrap();
        assert_eq!(diagnostics["crash_dump_included"], true);
    }

    #[test]
    fn a_log_longer_than_the_tail_starts_at_a_whole_line() {
        let (f, _) = fixture();
        let line = "2026-11-26T10:00:00Z  INFO butterpollo: tick peer=10.0.0.5\n";
        let repeat = (LOG_TAIL as usize / line.len()) + 10;
        std::fs::write(
            f.host.directory.join("logs/butterpollo.log"),
            format!("cut{}", line.repeat(repeat)),
        )
        .unwrap();
        let path = bundle(&f.host, false).unwrap();
        let log = &entries(&path)[0].1;
        assert!(
            log.starts_with("2026-11-26T10:00:00Z  INFO"),
            "{}",
            &log[..80]
        );
        assert!(!log.contains("10.0.0.5"));
    }
}
