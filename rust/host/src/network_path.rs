//! On-demand, bounded route diagnosis. Never starts or configures a VPN.
use butterpollo_core::network_path::{MAX_STATUS_BYTES, NetworkPath, from_tailscale_status};
use std::{
    net::IpAddr,
    process::Stdio,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

type CachedPath = Option<(Instant, IpAddr, NetworkPath)>;
static CACHE: OnceLock<Mutex<CachedPath>> = OnceLock::new();

pub async fn inspect(remote: IpAddr) -> NetworkPath {
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(None)).try_lock() else {
        return NetworkPath::unknown();
    };
    if let Some((at, peer, report)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return if *peer == remote {
            report.clone()
        } else {
            NetworkPath::unknown()
        };
    }
    let report = query(remote).await.unwrap_or_else(NetworkPath::unknown);
    *cache = Some((Instant::now(), remote, report.clone()));
    report
}

async fn query(remote: IpAddr) -> Option<NetworkPath> {
    // Unit tests exercise routing/authentication without talking to a local VPN.
    // The status parser is separately covered with captured-shape JSON fixtures.
    if cfg!(test) {
        return None;
    }
    // Use the standard protected installation path rather than the service's
    // current directory/PATH. No command-line arguments originate from clients.
    let executable = std::path::PathBuf::from(std::env::var_os("ProgramFiles")?)
        .join("Tailscale")
        .join("tailscale.exe");
    if !executable.is_file() {
        return None;
    }
    let mut command = Command::new(executable);
    command
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_STATUS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > MAX_STATUS_BYTES {
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        Some(from_tailscale_status(&bytes, remote))
    })
    .await
    .ok()
    .flatten();
    // kill_on_drop handles timeout, oversized output and early read errors.
    result
}
