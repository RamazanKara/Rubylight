use crate::state::{Host, Shared};
use axum::http::StatusCode;
use butterpollo_core::session::Role;
use butterpollo_windows::telemetry::Collector;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Weak, atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Error = (StatusCode, &'static str);
pub struct Request {
    client: String,
    reply: tokio::sync::oneshot::Sender<Result<Value, Error>>,
}

fn options(h: &Host, client: &str) -> Result<(Duration, Vec<String>), Error> {
    let config = h.config.read().unwrap();
    if !config.boolean("hardware_metrics", false) {
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

pub async fn sample(h: &Shared, client: String) -> Result<Value, Error> {
    let (_, selected) = options(h, &client)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    {
        let mut worker = h.hardware_metrics.lock().unwrap();
        if worker.is_none() {
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
            *worker = Some(sender);
        }
        worker
            .as_ref()
            .unwrap()
            .try_send(Request {
                client: client.clone(),
                reply,
            })
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Hardware metrics are busy"))?;
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

fn run(host: Weak<Host>, requests: mpsc::Receiver<Request>) {
    let mut collector = None;
    let mut selected = Vec::new();
    let mut cached: Option<(Instant, Value)> = None;
    let mut requester = String::new();
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
        let Some(request) = request else {
            // No background polling: the client must keep requesting samples.
            if options(&h, &requester).is_err() {
                collector = None;
                cached = None;
            }
            continue;
        };
        if request.reply.is_closed() {
            continue;
        }
        let (interval, metrics) = match options(&h, &request.client) {
            Ok(options) => options,
            Err(error) => {
                let _ = request.reply.send(Err(error));
                continue;
            }
        };
        requester = request.client;
        if collector.is_none() || selected != metrics {
            // ADLX has one process-wide lifetime; release it before reopening.
            drop(collector.take());
            collector = Some(Collector::new(&metrics));
            selected = metrics;
            cached = None;
        }
        if cached
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= interval)
        {
            let sample = collector.as_mut().unwrap().sample();
            let mut value = serde_json::to_value(sample).unwrap();
            value["version"] = json!(1);
            value["sampled_at_unix_ms"] = json!(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            );
            cached = Some((Instant::now(), value));
        }
        let mut value = cached.as_ref().unwrap().1.clone();
        value["interval_ms"] = json!(interval.as_millis() as u64);
        let _ = request.reply.send(Ok(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    fn metrics_require_opt_in_and_the_requesting_clients_active_video_stream() {
        let f = Fixture::new();
        let client = f.client(0x0200_0000);
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
}
