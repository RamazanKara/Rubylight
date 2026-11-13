use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::session::Role;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) struct Received {
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// A receiver answering its requests with `statuses` in turn (the last
/// repeats); 0 never answers. The URL has a path and a query.
pub(crate) async fn receiver(statuses: Vec<u16>) -> (String, mpsc::UnboundedReceiver<Received>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/hook?token=private",
        listener.local_addr().unwrap()
    );
    let (sender, received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut answered = 0;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let status = statuses[answered.min(statuses.len() - 1)];
            answered += 1;
            let sender = sender.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buffer = [0; 4096];
                let (headers, length) = loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..read]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8(data[..end].to_vec()).unwrap();
                        let headers = head
                            .lines()
                            .skip(1)
                            .filter_map(|line| line.split_once(':'))
                            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
                            .collect::<HashMap<_, _>>();
                        data.drain(..end + 4);
                        let length = headers["content-length"].parse::<usize>().unwrap();
                        break (headers, length);
                    }
                };
                while data.len() < length {
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(read, 0, "the body ended early");
                    data.extend_from_slice(&buffer[..read]);
                }
                let _ = sender.send(Received {
                    headers,
                    body: data,
                });
                if status == 0 {
                    std::future::pending::<()>().await;
                }
                let answer = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                stream.write_all(answer.as_bytes()).await.unwrap();
            });
        }
    });
    (url, received)
}

const QUICK: Policy = Policy {
    timeout: Duration::from_secs(5),
    attempts: 3,
    backoff: Duration::from_millis(10),
    max_age: Duration::from_millis(webhook::REPLAY_WINDOW_MS),
};
const HOST: DeliveryHost<'static> = DeliveryHost {
    name: "Test PC",
    unique_id: "host-id",
};

fn hook(url: &str, events: &[&str]) -> Webhook {
    Webhook {
        url: url.into(),
        events: events.iter().map(|e| e.to_string()).collect(),
        secret: "receiver secret".into(),
    }
}

/// The hook's status once a delivery has finished.
async fn settled(webhooks: &Webhooks, url: &str) -> Status {
    for _ in 0..1000 {
        if let Some(status) = webhooks.status(url) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no delivery finished");
}

#[tokio::test]
async fn a_name_that_resolves_to_this_pc_is_refused_at_delivery() {
    let (url, mut received) = receiver(vec![204]).await;
    let port = url::Url::parse(&url).unwrap().port().unwrap();
    // The console refuses "localhost" by name; any other name pointing
    // here (nip.io, a rebinding domain) only shows when it resolves.
    let webhooks = Webhooks::new(QUICK).unwrap();
    let status = webhooks
        .test(&hook(&format!("http://localhost:{port}/hook"), &[]), HOST)
        .await;
    assert!(!status.ok);
    assert_eq!(status.http_status, None);
    assert!(received.try_recv().is_err());
    let addresses = [
        "127.0.0.1:80",
        "[::1]:80",
        "169.254.169.254:80",
        "[fe80::1]:80",
        "0.0.0.0:80",
        "[::ffff:127.0.0.1]:80",
        "192.168.4.20:80",
        "[fd00::20]:80",
    ]
    .map(|a| a.parse::<std::net::SocketAddr>().unwrap());
    assert_eq!(allowed_addresses(addresses), &addresses[6..]);
}
#[tokio::test]
async fn a_delivery_is_signed_so_the_receiver_can_verify_it() {
    let (url, mut received) = receiver(vec![204]).await;
    let webhooks = Webhooks::new(QUICK).unwrap();
    let hooks = [hook(&url, &["app.started", "app.stopped"])];
    let queued = webhooks.emit(&hooks, HOST, "app.started", json!({"app": {"id": 7}}));
    assert_eq!(queued, 1);
    let request = received.recv().await.unwrap();
    // The receiver's checks, as docs/webhooks.md describes them.
    let expected = format!(
        "{}{}",
        webhook::SIGNATURE_PREFIX,
        webhook::sign("receiver secret", &request.body)
    );
    assert_eq!(request.headers["x-rubylight-signature"], expected);
    assert_eq!(request.headers["content-type"], "application/json");
    assert_eq!(request.headers["x-rubylight-event"], "app.started");
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["event"], "app.started");
    assert_eq!(
        body["delivery"],
        request.headers["x-rubylight-delivery"].as_str()
    );
    assert_eq!(
        body["time_unix_ms"].to_string(),
        request.headers["x-rubylight-timestamp"]
    );
    let age = now_ms().abs_diff(body["time_unix_ms"].as_u64().unwrap());
    assert!(age < webhook::REPLAY_WINDOW_MS, "{age}");
    assert_eq!(
        body["host"],
        json!({"name": "Test PC", "unique_id": "host-id"})
    );
    assert_eq!(body["data"], json!({"app": {"id": 7}}));
    let status = settled(&webhooks, &url).await;
    assert!(status.ok, "{status:?}");
    assert_eq!((status.http_status, status.attempts), (Some(204), 1));
    assert_eq!(status.event, "app.started");
}

#[tokio::test]
async fn an_event_no_hook_subscribes_to_sends_nothing() {
    let webhooks = Webhooks::new(QUICK).unwrap();
    let hooks = [hook("http://127.0.0.1:9/", &["pairing.paired"])];
    assert_eq!(webhooks.emit(&hooks, HOST, "app.started", json!({})), 0);
    assert!(webhooks.status("http://127.0.0.1:9/").is_none());
}

#[tokio::test]
async fn a_server_error_is_retried_with_the_same_delivery() {
    let (url, mut received) = receiver(vec![503, 200]).await;
    let webhooks = Webhooks::new(QUICK).unwrap();
    let hooks = [hook(&url, &["session.started"])];
    webhooks.emit(&hooks, HOST, "session.started", json!({"n": 1}));
    let first = received.recv().await.unwrap();
    let second = received.recv().await.unwrap();
    // Each try is stamped and signed anew, keeping the delivery id to de-duplicate by.
    let mut times = Vec::new();
    for request in [&first, &second] {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            request.headers["x-rubylight-signature"],
            format!("sha256={}", webhook::sign("receiver secret", &request.body))
        );
        assert_eq!(
            body["time_unix_ms"].to_string(),
            request.headers["x-rubylight-timestamp"]
        );
        assert_eq!(
            body["delivery"],
            first.headers["x-rubylight-delivery"].as_str()
        );
        assert_eq!(body["data"], json!({"n": 1}));
        times.push(body["time_unix_ms"].as_u64().unwrap());
    }
    assert_eq!(
        first.headers["x-rubylight-delivery"],
        second.headers["x-rubylight-delivery"]
    );
    // The retry waited 10 ms.
    assert!(times[1] > times[0], "{times:?}");
    let status = settled(&webhooks, &url).await;
    assert!(status.ok, "{status:?}");
    assert_eq!((status.http_status, status.attempts), (Some(200), 2));
}

#[tokio::test]
async fn a_delivery_queued_past_the_replay_window_is_dropped() {
    // A receiver that never answers holds the queue; what waited behind it is too old.
    let (url, mut received) = receiver(vec![0]).await;
    let webhooks = Webhooks::new(Policy {
        timeout: Duration::from_millis(300),
        attempts: 1,
        max_age: Duration::from_millis(100),
        ..QUICK
    })
    .unwrap();
    let hooks = [hook(&url, &["app.started", "app.stopped"])];
    webhooks.emit(&hooks, HOST, "app.started", json!({}));
    webhooks.emit(&hooks, HOST, "app.stopped", json!({}));
    received.recv().await.unwrap();
    let mut status = None;
    for _ in 0..1000 {
        status = webhooks.status(&url).filter(|s| s.event == "app.stopped");
        if status.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let status = status.expect("the second delivery finished");
    assert!(!status.ok);
    assert_eq!((status.http_status, status.attempts), (None, 0));
    assert!(
        status.error.as_deref().unwrap().contains("replay window"),
        "{status:?}"
    );
    // Never sent.
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn a_refusal_is_not_retried_and_shows_as_failed() {
    let (url, _received) = receiver(vec![401, 200]).await;
    let webhooks = Webhooks::new(QUICK).unwrap();
    let hooks = [hook(&url, &["session.ended"])];
    webhooks.emit(&hooks, HOST, "session.ended", json!({}));
    let status = settled(&webhooks, &url).await;
    assert!(!status.ok);
    assert_eq!((status.http_status, status.attempts), (Some(401), 1));
    assert!(status.error.unwrap().contains("401"));
}

#[tokio::test]
async fn a_receiver_that_never_answers_times_out() {
    let (url, _received) = receiver(vec![0]).await;
    let webhooks = Webhooks::new(Policy {
        timeout: Duration::from_millis(200),
        attempts: 2,
        backoff: Duration::from_millis(10),
        ..QUICK
    })
    .unwrap();
    let hooks = [hook(&url, &["app.stopped"])];
    webhooks.emit(&hooks, HOST, "app.stopped", json!({}));
    let status = settled(&webhooks, &url).await;
    assert!(!status.ok);
    assert_eq!((status.http_status, status.attempts), (None, 2));
    let error = status.error.unwrap();
    assert!(error.contains("no answer within 200 ms"), "{error}");
    assert!(
        !error.contains("private"),
        "the URL's query stays out: {error}"
    );
}

#[tokio::test]
async fn a_full_queue_drops_new_deliveries_without_waiting() {
    let (url, _received) = receiver(vec![0]).await;
    let webhooks = Webhooks::new(QUICK).unwrap();
    let hooks = [hook(&url, &["app.started"])];
    let started = std::time::Instant::now();
    let queued: usize = (0..QUEUE + 10)
        .map(|_| webhooks.emit(&hooks, HOST, "app.started", json!({})))
        .sum();
    assert!(started.elapsed() < Duration::from_secs(1));
    // The task may already hold the first delivery, freeing one place.
    assert!((QUEUE..=QUEUE + 1).contains(&queued), "{queued}");
    let status = webhooks.status(&url).unwrap();
    assert!(!status.ok);
    assert_eq!(status.attempts, 0);
    assert_eq!(status.error.as_deref(), Some("the hook's queue is full"));
}

#[tokio::test]
async fn a_deleted_hooks_queued_deliveries_are_dropped() {
    let (url, mut received) = receiver(vec![0]).await;
    let webhooks = Webhooks::new(Policy {
        timeout: Duration::from_millis(300),
        attempts: 2,
        ..QUICK
    })
    .unwrap();
    let hooks = [hook(&url, &["app.started"])];
    for _ in 0..3 {
        assert_eq!(webhooks.emit(&hooks, HOST, "app.started", json!({})), 1);
    }
    // The first is being sent: it gets no second try, and the other two never go.
    received.recv().await.unwrap();
    webhooks.forget(&url);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(received.try_recv().is_err());
    assert!(webhooks.status(&url).is_none());
    // A hook added again at that URL starts afresh.
    let (again, mut reached) = receiver(vec![200]).await;
    let hooks = [hook(&again, &["app.started"])];
    webhooks.emit(&hooks, HOST, "app.started", json!({}));
    webhooks.forget(&again);
    let hooks = [hook(&again, &["app.started"])];
    webhooks.emit(&hooks, HOST, "app.started", json!({}));
    reached.recv().await.unwrap();
    assert!(settled(&webhooks, &again).await.ok);
}

#[test]
fn without_a_runtime_a_delivery_fails_loudly() {
    let webhooks = Webhooks::new(QUICK).unwrap();
    let url = "http://127.0.0.1:9/";
    let hooks = [hook(url, &["app.started"])];
    assert_eq!(webhooks.emit(&hooks, HOST, "app.started", json!({})), 0);
    let status = webhooks.status(url).unwrap();
    assert_eq!(
        status.error.as_deref(),
        Some("no runtime runs webhook deliveries")
    );
    webhooks.forget(url);
    assert!(webhooks.status(url).is_none());
}

#[test]
fn logs_name_a_hook_by_its_origin_only() {
    assert_eq!(
        origin("https://example.com:8443/a/b?token=x#f"),
        "https://example.com:8443"
    );
    assert_eq!(origin("http://127.0.0.1/secret"), "http://127.0.0.1");
    assert_eq!(origin("not a url"), "<invalid URL>");
}

#[tokio::test]
async fn host_events_carry_the_documented_data() {
    let (url, mut received) = receiver(vec![200]).await;
    let f = Fixture::new();
    let all: Vec<&str> = webhook::EVENTS.to_vec();
    let hooks = serde_json::to_string(&[hook(&url, &all)]).unwrap();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("webhooks".into(), hooks);
    let client = f.client(0);
    let device = json!({"uuid": "parity-device", "name": "Parity device"});

    f.host
        .app_started_webhook(Some((7, "Desktop".into(), client.uuid.clone())));
    let body: Value = serde_json::from_slice(&received.recv().await.unwrap().body).unwrap();
    assert_eq!(body["event"], "app.started");
    assert_eq!(
        body["data"],
        json!({"app": {"id": 7, "name": "Desktop"}, "device": device})
    );
    let unique_id = f.host.paired.read().unwrap().unique_id.clone();
    assert_eq!(body["host"]["unique_id"], unique_id.as_str());

    // A placeholder waiting for a sign-in raises nothing yet.
    f.host.app_started_webhook(None);
    let session = f.session(client.clone(), Role::Stream);
    f.host.session_webhook("session.started", &session);
    let body: Value = serde_json::from_slice(&received.recv().await.unwrap().body).unwrap();
    assert_eq!(body["event"], "session.started");
    assert_eq!(
        body["data"],
        json!({
            "session": session.launch.id,
            "device": device,
            "app_id": session.launch.app_id,
            "role": "stream",
            "transport": "moonlight",
        })
    );
    f.host.session_webhook("session.ended", &session);
    let body: Value = serde_json::from_slice(&received.recv().await.unwrap().body).unwrap();
    assert_eq!(body["event"], "session.ended");
    assert_eq!(body["data"]["failed"], false);

    assert_eq!(f.host.webhook_device(""), Value::Null);
    assert_eq!(
        f.host.webhook_device("unknown"),
        json!({"uuid": "unknown", "name": ""})
    );
}
