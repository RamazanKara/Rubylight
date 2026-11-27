use crate::state::{Host, Shared};
use axum::http::StatusCode;
use butterpollo_session::session::Role;
use butterpollo_win_graphics::telemetry::Collector;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, PoisonError, Weak, atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Error = (StatusCode, &'static str);
/// Latest telemetry worker GPU utilization reading for the active queue-clamped stream's adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedGpuBusy {
    adapter: String,
    packed: u64,
}

#[derive(Default)]
struct GpuBusyCache {
    entry: Option<CachedGpuBusy>,
}

impl GpuBusyCache {
    fn clear(&mut self) {
        self.entry = None;
    }

    fn store(&mut self, adapter: &str, unix_ms: u64, pct: u8) {
        self.entry = Some(CachedGpuBusy {
            adapter: adapter.to_owned(),
            packed: pack_gpu_busy(unix_ms, pct),
        });
    }

    /// Returns `(reading_within_max_age, is_fresh_for_throttle)` only when the cached entry
    /// belongs to `adapter`.
    fn read(&self, adapter: &str, now_unix_ms: u64) -> (Option<u8>, bool) {
        let Some(entry) = self.entry.as_ref() else {
            return (None, false);
        };
        if !entry.adapter.trim().eq_ignore_ascii_case(adapter.trim()) {
            return (None, false);
        }
        let fresh =
            (entry.packed & 0xFF) != 0 && now_unix_ms.saturating_sub(entry.packed >> 8) < 900;
        (unpack_gpu_busy_at(entry.packed, now_unix_ms), fresh)
    }
}

/// Active stream whose GPU adapter owns `LAST_GPU_BUSY` during dynamic queue clamping.
#[derive(Default)]
struct TargetAdapter {
    stream: Option<(String, String)>,
}

impl TargetAdapter {
    fn clear(&mut self) {
        self.stream = None;
    }

    /// Updates the tracked `(client, adapter)` from an internal clamping poll. Returns `true`
    /// when the tracked adapter changed.
    fn set_internal(&mut self, client: &str, adapter: &str) -> bool {
        let changed = self
            .stream
            .as_ref()
            .is_none_or(|(_, cur)| !cur.trim().eq_ignore_ascii_case(adapter.trim()));
        if changed {
            self.stream = Some((client.to_owned(), adapter.to_owned()));
        } else if let Some((cur_client, _)) = self.stream.as_mut()
            && cur_client != client
        {
            *cur_client = client.to_owned();
        }
        changed
    }

    /// Drops the tracked target if its stream is no longer active; returns the active adapter
    /// name when one is still live.
    fn active(&mut self, is_active: impl FnOnce(&str) -> bool) -> Option<&str> {
        let still_live = self
            .stream
            .as_ref()
            .is_some_and(|(client, _)| is_active(client));
        if !still_live {
            self.stream = None;
            return None;
        }
        self.stream.as_ref().map(|(_, adapter)| adapter.as_str())
    }
}

static LAST_GPU_BUSY: Mutex<GpuBusyCache> = Mutex::new(GpuBusyCache { entry: None });
const GPU_BUSY_MAX_AGE_MS: u64 = 3_000;
const INTERNAL_GPU_INTERVAL: Duration = Duration::from_secs(1);

pub struct Request {
    client: String,
    gpu_adapter: String,
    internal_gpu_only: bool,
    reply: Option<tokio::sync::oneshot::Sender<Result<Value, Error>>>,
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn gpu_usage_for_adapter(value: &Value, adapter_name: &str) -> Option<u8> {
    let gpus = value["gpus"].as_array()?;
    let pct_of = |gpu: &Value| {
        gpu["metrics"]["gpu_usage_percent"]
            .as_f64()
            .filter(|pct| pct.is_finite() && *pct >= 0.0)
            .map(|pct| pct.round().clamp(0.0, 100.0) as u8)
    };
    let target = adapter_name.trim();
    if !target.is_empty() {
        let matched = gpus
            .iter()
            .filter(|gpu| {
                gpu["name"]
                    .as_str()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case(target))
            })
            .filter_map(pct_of)
            .max();
        if matched.is_some() || gpus.iter().any(|gpu| gpu["name"].as_str().is_some()) {
            return matched;
        }
    }
    gpus.iter().filter_map(pct_of).max()
}

fn filter_client_metrics(mut value: Value, client_selected: &[String]) -> Value {
    if client_selected.is_empty() || client_selected.iter().any(|m| m == "gpu_usage_percent") {
        return value;
    }
    if !client_selected.iter().any(|m| m.starts_with("gpu_")) {
        value["gpus"] = json!([]);
        return value;
    }
    if let Some(gpus) = value["gpus"].as_array_mut() {
        gpus.retain_mut(|gpu| {
            if let Some(metrics) = gpu["metrics"].as_object_mut() {
                metrics.remove("gpu_usage_percent");
                return !metrics.is_empty() || gpu["id"].as_i64().is_some_and(|id| id >= 0);
            }
            true
        });
    }
    value
}

fn pack_gpu_busy(unix_ms: u64, pct: u8) -> u64 {
    (unix_ms << 8) | u64::from(pct.min(100) + 1)
}

fn unpack_gpu_busy_at(packed: u64, now_unix_ms: u64) -> Option<u8> {
    let raw = (packed & 0xFF) as u8;
    if raw == 0 {
        return None;
    }
    let at_ms = packed >> 8;
    (now_unix_ms.saturating_sub(at_ms) <= GPU_BUSY_MAX_AGE_MS).then_some(raw - 1)
}

/// Clears any cached internal GPU utilization reading once dynamic queue clamping turns off.
pub fn clear_gpu_busy() {
    LAST_GPU_BUSY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

fn store_gpu_busy(adapter: &str, unix_ms: u64, pct: u8) {
    LAST_GPU_BUSY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .store(adapter, unix_ms, pct);
}

fn read_gpu_busy_for(adapter: &str, now_ms: u64) -> (Option<u8>, bool) {
    let Ok(cache) = LAST_GPU_BUSY.try_lock() else {
        return (None, false);
    };
    cache.read(adapter, now_ms)
}

/// Non-blocking: triggers an off-thread `gpu_usage_percent` sample on the telemetry worker for
/// `client` and `gpu_adapter` (throttled to ~1 Hz when a fresh sample is already cached) and
/// returns the freshest recent reading (within 3 s), if any.
pub fn poll_and_read_gpu_busy(h: &Shared, client: &str, gpu_adapter: &str) -> Option<u8> {
    if !butterpollo_win_io::limiter::queue_clamping() {
        clear_gpu_busy();
        return None;
    }
    let now_ms = unix_ms_now();
    let (reading, fresh) = read_gpu_busy_for(gpu_adapter, now_ms);
    if fresh {
        return reading;
    }
    if let Ok(mut worker) = h.hardware_metrics.try_lock() {
        let req = || Request {
            client: client.to_owned(),
            gpu_adapter: gpu_adapter.to_owned(),
            internal_gpu_only: true,
            reply: None,
        };
        let sent = if let Some(sender) = worker.as_ref() {
            sender.try_send(req())
        } else if let Ok(sender) = spawn_worker(h) {
            *worker = Some(sender.clone());
            sender.try_send(req())
        } else {
            Ok(())
        };
        if matches!(sent, Err(mpsc::TrySendError::Disconnected(_))) {
            *worker = spawn_worker(h).ok();
            if let Some(sender) = worker.as_ref() {
                let _ = sender.try_send(req());
            }
        }
    }
    read_gpu_busy_for(gpu_adapter, now_ms).0
}

fn options(h: &Host, client: &str) -> Result<(Duration, Vec<String>), Error> {
    let config = h.config.read().unwrap();
    if !config.hardware_metrics() {
        return Err((StatusCode::FORBIDDEN, "Hardware metrics are disabled"));
    }
    let options = (
        Duration::from_millis(config.hardware_metrics_interval()),
        config.list("hardware_metrics_selection"),
    );
    drop(config);
    if h.stop.load(Ordering::Acquire)
        || !h.sessions.lock().unwrap().active.values().any(|session| {
            session.launch.client.uuid == client
                && session.launch.role != Role::InputOnly
                && !session.stopping()
        })
    {
        return Err((StatusCode::CONFLICT, "An active video stream is required"));
    }
    Ok(options)
}

fn spawn_worker(h: &Shared) -> Result<mpsc::SyncSender<Request>, Error> {
    let (sender, receiver) = mpsc::sync_channel(4);
    let host = Arc::downgrade(h);
    std::thread::Builder::new()
        .name("hardware-metrics".into())
        .spawn(move || run(host, receiver))
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics are unavailable",
            )
        })?;
    Ok(sender)
}

pub async fn sample(h: &Shared, client: String) -> Result<Value, Error> {
    let (_, selected) = options(h, &client)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    {
        let mut worker = h.hardware_metrics.lock().unwrap();
        let request = Request {
            client: client.clone(),
            gpu_adapter: String::new(),
            internal_gpu_only: false,
            reply: Some(reply),
        };
        let send = if let Some(sender) = worker.as_ref() {
            sender.try_send(request)
        } else {
            let sender = spawn_worker(h)?;
            *worker = Some(sender.clone());
            sender.try_send(request)
        };
        match send {
            Ok(()) => {}
            Err(mpsc::TrySendError::Disconnected(request)) => {
                *worker = None;
                let sender = spawn_worker(h)?;
                *worker = Some(sender.clone());
                sender
                    .try_send(request)
                    .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Hardware metrics are busy"))?;
            }
            Err(mpsc::TrySendError::Full(_)) => {
                return Err((StatusCode::SERVICE_UNAVAILABLE, "Hardware metrics are busy"));
            }
        }
    }
    let value = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics timed out",
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics are unavailable",
            )
        })??;
    // A driver call may outlive its stream or a console setting change.
    if options(h, &client)?.1 != selected {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Hardware metric selection changed",
        ));
    }
    Ok(value)
}

fn active_video_stream(h: &Host, client: &str) -> bool {
    !h.stop.load(Ordering::Acquire)
        && h.sessions.lock().unwrap().active.values().any(|session| {
            session.launch.client.uuid == client
                && session.launch.role != Role::InputOnly
                && !session.stopping()
        })
}

fn run(host: Weak<Host>, requests: mpsc::Receiver<Request>) {
    let mut collector = None;
    let mut selected = Vec::new();
    let mut cached: Option<(Instant, Value)> = None;
    let mut requester = String::new();
    let mut target_adapter = TargetAdapter::default();
    loop {
        let received = if collector.is_some() {
            requests.recv_timeout(Duration::from_millis(250))
        } else {
            requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let request = match received {
            Ok(request) => Some(request),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(h) = host.upgrade() else { return };
        if h.stop.load(Ordering::Acquire) {
            return;
        }
        let clamping = butterpollo_win_io::limiter::queue_clamping();
        if !clamping {
            clear_gpu_busy();
        }
        let Some(request) = request else {
            // Retain the collector while either the opted-in client API or dynamic queue clamp
            // still needs samples for an active video stream.
            let target_live = clamping
                && target_adapter
                    .active(|client| active_video_stream(&h, client))
                    .is_some();
            let still_valid = options(&h, &requester).is_ok()
                || (active_video_stream(&h, &requester) && clamping)
                || target_live;
            if !still_valid {
                collector = None;
                cached = None;
                requester.clear();
                target_adapter.clear();
                clear_gpu_busy();
            } else if !target_live {
                clear_gpu_busy();
            }
            continue;
        };
        if request.reply.as_ref().is_some_and(|r| r.is_closed()) {
            continue;
        }
        let mut adapter_changed = false;
        let (client_interval, client_selected) = if request.internal_gpu_only {
            if !active_video_stream(&h, &request.client) {
                continue;
            }
            adapter_changed = target_adapter.set_internal(&request.client, &request.gpu_adapter);
            if adapter_changed {
                clear_gpu_busy();
            }
            match options(&h, &request.client) {
                Ok((interval, metrics)) => (interval, metrics),
                Err(_) => (INTERNAL_GPU_INTERVAL, vec!["gpu_usage_percent".into()]),
            }
        } else {
            match options(&h, &request.client) {
                Ok(options) => options,
                Err(error) => {
                    if let Some(reply) = request.reply {
                        let _ = reply.send(Err(error));
                    }
                    continue;
                }
            }
        };
        let mut collector_metrics = client_selected.clone();
        if (request.internal_gpu_only || clamping)
            && !collector_metrics.is_empty()
            && !collector_metrics.iter().any(|m| m == "gpu_usage_percent")
        {
            collector_metrics.push("gpu_usage_percent".into());
        }
        let sample_interval = if request.internal_gpu_only || clamping {
            client_interval.min(INTERNAL_GPU_INTERVAL)
        } else {
            client_interval
        };
        requester = request.client;
        if collector.is_none() || selected != collector_metrics {
            // ADLX has one process-wide lifetime; release it before reopening.
            drop(collector.take());
            collector = Some(Collector::new(&collector_metrics));
            selected = collector_metrics;
            cached = None;
        }
        if cached
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= sample_interval)
        {
            let sample = collector.as_mut().unwrap().sample();
            let now_ms = unix_ms_now();
            let mut value = serde_json::to_value(sample).unwrap();
            value["version"] = json!(1);
            value["sampled_at_unix_ms"] = json!(now_ms);
            if clamping {
                if let Some(adapter) =
                    target_adapter.active(|client| active_video_stream(&h, client))
                {
                    if let Some(busy) = gpu_usage_for_adapter(&value, adapter) {
                        store_gpu_busy(adapter, now_ms, busy);
                    }
                } else {
                    clear_gpu_busy();
                }
            }
            cached = Some((Instant::now(), value));
        } else if clamping
            && adapter_changed
            && let Some((_, value)) = cached.as_ref()
            && let Some(adapter) = target_adapter.active(|client| active_video_stream(&h, client))
            && let Some(busy) = gpu_usage_for_adapter(value, adapter)
        {
            let at_ms = value["sampled_at_unix_ms"]
                .as_u64()
                .unwrap_or_else(unix_ms_now);
            store_gpu_busy(adapter, at_ms, busy);
        }
        if let Some(reply) = request.reply {
            let mut value =
                filter_client_metrics(cached.as_ref().unwrap().1.clone(), &client_selected);
            value["interval_ms"] = json!(client_interval.as_millis() as u64);
            let _ = reply.send(Ok(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    fn gpu_busy_matches_stream_adapter_filters_unrequested_metrics_and_bounds_age() {
        let val = json!({
            "cpu": { "cpu_usage_percent": 21.0 },
            "gpus": [
                {
                    "id": 0,
                    "name": "AMD Radeon 780M Graphics",
                    "metrics": { "gpu_usage_percent": 99.2, "gpu_temperature_c": 52.0 }
                },
                {
                    "id": -10,
                    "name": "NVIDIA GeForce RTX 4070 Laptop GPU",
                    "metrics": { "gpu_usage_percent": 61.4 }
                }
            ]
        });
        // Matches the active stream's dGPU rather than taking 99% from the iGPU.
        assert_eq!(
            gpu_usage_for_adapter(&val, "NVIDIA GeForce RTX 4070 Laptop GPU"),
            Some(61)
        );
        assert_eq!(
            gpu_usage_for_adapter(&val, "AMD Radeon 780M Graphics"),
            Some(99)
        );
        assert_eq!(gpu_usage_for_adapter(&val, "Unknown Adapter"), None);
        assert_eq!(gpu_usage_for_adapter(&val, ""), Some(99));

        // Strips unrequested `gpu_usage_percent` from external API responses.
        let cpu_only = filter_client_metrics(val.clone(), &["cpu_usage_percent".into()]);
        assert_eq!(cpu_only["gpus"], json!([]));
        let temp_only = filter_client_metrics(val.clone(), &["gpu_temperature_c".into()]);
        assert_eq!(temp_only["gpus"].as_array().unwrap().len(), 1);
        assert!(
            temp_only["gpus"][0]["metrics"]
                .get("gpu_usage_percent")
                .is_none()
        );
        assert_eq!(temp_only["gpus"][0]["metrics"]["gpu_temperature_c"], 52.0);

        let packed = pack_gpu_busy(100_000, 99);
        assert_eq!(unpack_gpu_busy_at(packed, 102_500), Some(99));
        assert_eq!(unpack_gpu_busy_at(packed, 103_000), Some(99));
        assert_eq!(unpack_gpu_busy_at(packed, 103_001), None);
        assert_eq!(unpack_gpu_busy_at(0, 100_000), None);
    }

    #[test]
    fn metrics_require_opt_in_and_the_requesting_clients_active_video_stream() {
        let f = Fixture::new();
        let client = f.client(0x0200_0000);
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("hardware_metrics".into(), "false".into());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("hardware_metrics".into(), "true".into());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap_err().0,
            StatusCode::CONFLICT
        );
        let input = f.session(client.clone(), Role::InputOnly);
        assert!(options(&f.host, &client.uuid).is_err());
        let stream = f.session(client.clone(), Role::Stream);
        assert!(options(&f.host, "another-client").is_err());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap().0,
            Duration::from_secs(1)
        );
        stream.stop();
        input.stop();
        assert!(options(&f.host, &client.uuid).is_err());
        assert!(f.host.hardware_metrics.lock().unwrap().is_none());
    }

    #[test]
    fn target_adapter_clears_and_cache_does_not_starve_or_cross_adapters() {
        let mut target = TargetAdapter::default();
        // Before any internal poll sets a stream adapter, external samples have no target.
        assert_eq!(target.active(|_| true), None);

        // Stream 1 registers GPU A.
        assert!(target.set_internal("client-a", "NVIDIA GeForce RTX 4070 Laptop GPU"));
        assert!(!target.set_internal("client-a", "nvidia geforce rtx 4070 laptop gpu"));
        assert_eq!(
            target.active(|c| c == "client-a"),
            Some("NVIDIA GeForce RTX 4070 Laptop GPU")
        );

        // Cache stores GPU A's utilization; GPU B must NOT see GPU A's reading or be throttled.
        let mut cache = GpuBusyCache::default();
        cache.store("NVIDIA GeForce RTX 4070 Laptop GPU", 100_000, 61);
        assert_eq!(
            cache.read("NVIDIA GeForce RTX 4070 Laptop GPU", 100_500),
            (Some(61), true)
        );
        assert_eq!(
            cache.read("AMD Radeon 780M Graphics", 100_500),
            (None, false)
        );

        // When Stream 1 ends, `active` drops the stale target instead of keeping GPU A.
        assert_eq!(target.active(|c| c == "client-b"), None);
        assert_eq!(target.active(|_| true), None);

        // Clearing resets target as on `!still_valid`.
        assert!(target.set_internal("client-b", "AMD Radeon 780M Graphics"));
        target.clear();
        assert_eq!(target.active(|_| true), None);
    }
}
