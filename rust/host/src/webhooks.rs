//! Sends the events of the `webhooks` setting ([`butterpollo_core::webhook`]).
//!
//! Nothing here runs on a stream's path: an event is serialized once and put
//! on its hook's bounded queue with `try_send`, so a full queue drops the
//! delivery (logged, and shown in the console) instead of waiting. Each hook
//! has its own task, so a slow receiver holds up only its own deliveries,
//! which arrive in order. A delivery is tried [`Policy::attempts`] times, each
//! within [`Policy::timeout`], waiting [`Policy::backoff`] doubled between
//! tries; retries resend the same body, signature and delivery id. A failure
//! is logged with the hook's origin only, since a URL's path or query may hold
//! a token, and the secret is never logged.
use crate::state::Host;
use butterpollo_core::webhook::{self, Delivery, DeliveryHost, Webhook};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Deliveries waiting per hook before new ones are dropped.
pub const QUEUE: usize = 32;
/// Hooks the console may add.
pub const MAX_HOOKS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// For one attempt, connecting included.
    pub timeout: Duration,
    pub attempts: u32,
    /// The wait after the first failed attempt, doubled after each next one.
    pub backoff: Duration,
}

impl Policy {
    /// At most 4 tries over about 27 s, well within
    /// [`webhook::REPLAY_WINDOW_MS`].
    pub const DEFAULT: Self = Self {
        timeout: Duration::from_secs(5),
        attempts: 4,
        backoff: Duration::from_secs(1),
    };
}

/// A hook's last delivery, for the console.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub event: String,
    pub delivery: String,
    pub time_unix_ms: u64,
    pub ok: bool,
    /// The receiver's last HTTP status, if it answered.
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub attempts: u32,
}

struct Job {
    hook: Webhook,
    event: String,
    delivery: String,
    time_unix_ms: u64,
    body: Arc<Vec<u8>>,
}

pub struct Webhooks {
    client: reqwest::Client,
    policy: Policy,
    /// The runtime of the thread that made this, for events raised on
    /// threads outside it.
    runtime: Option<tokio::runtime::Handle>,
    /// Each hook's queue, by URL.
    queues: Mutex<HashMap<String, mpsc::Sender<Job>>>,
    status: Arc<Mutex<HashMap<String, Status>>>,
}

impl Webhooks {
    pub fn new(policy: Policy) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            // A redirect could send the signed body somewhere else.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(policy.timeout)
            .timeout(policy.timeout)
            .user_agent(concat!("Rubylight-Webhooks/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            policy,
            runtime: tokio::runtime::Handle::try_current().ok(),
            queues: Mutex::default(),
            status: Arc::default(),
        })
    }

    /// Queues `event` for each of `hooks` that subscribes to it and returns
    /// how many took it. Never waits.
    pub fn emit(
        &self,
        hooks: &[Webhook],
        host: DeliveryHost<'_>,
        event: &str,
        data: Value,
    ) -> usize {
        let hooks: Vec<_> = hooks
            .iter()
            .filter(|hook| hook.events.iter().any(|e| e == event))
            .collect();
        if hooks.is_empty() {
            return 0;
        }
        let delivery = uuid::Uuid::new_v4().to_string();
        let time_unix_ms = now_ms();
        let body = match serde_json::to_vec(&Delivery {
            event,
            delivery: &delivery,
            time_unix_ms,
            host,
            data,
        }) {
            Ok(body) => Arc::new(body),
            Err(error) => {
                tracing::error!(%error, event, "a webhook body could not be written");
                return 0;
            }
        };
        let mut queued = 0;
        for hook in hooks {
            let job = Job {
                hook: hook.clone(),
                event: event.to_owned(),
                delivery: delivery.clone(),
                time_unix_ms,
                body: body.clone(),
            };
            match self.enqueue(job) {
                Ok(()) => queued += 1,
                Err(error) => {
                    tracing::warn!(
                        hook = %origin(&hook.url),
                        event,
                        %delivery,
                        error,
                        "webhook delivery dropped"
                    );
                    self.record(
                        &hook.url,
                        Status {
                            event: event.to_owned(),
                            delivery: delivery.clone(),
                            time_unix_ms,
                            ok: false,
                            http_status: None,
                            error: Some(error.to_owned()),
                            attempts: 0,
                        },
                    );
                }
            }
        }
        queued
    }

    fn enqueue(&self, job: Job) -> Result<(), &'static str> {
        let mut queues = self.queues.lock().unwrap();
        let job = match queues.get(&job.hook.url) {
            Some(queue) => match queue.try_send(job) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::TrySendError::Full(_)) => return Err("the hook's queue is full"),
                // Its task ended: start another.
                Err(mpsc::error::TrySendError::Closed(job)) => job,
            },
            None => job,
        };
        let runtime = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| self.runtime.clone())
            .ok_or("no runtime runs webhook deliveries")?;
        let (sender, mut receiver) = mpsc::channel::<Job>(QUEUE);
        let client = self.client.clone();
        let policy = self.policy;
        let status = self.status.clone();
        runtime.spawn(async move {
            while let Some(job) = receiver.recv().await {
                let result = deliver(&client, policy, &job).await;
                finish(&status, &job, result);
            }
        });
        let url = job.hook.url.clone();
        sender
            .try_send(job)
            .map_err(|_| "the hook's queue is full")?;
        queues.insert(url, sender);
        Ok(())
    }

    /// Sends [`webhook::TEST_EVENT`] to `hook` once, now, and returns how it went.
    pub async fn test(&self, hook: &Webhook, host: DeliveryHost<'_>) -> Status {
        let delivery = uuid::Uuid::new_v4().to_string();
        let time_unix_ms = now_ms();
        let body = serde_json::to_vec(&Delivery {
            event: webhook::TEST_EVENT,
            delivery: &delivery,
            time_unix_ms,
            host,
            data: serde_json::json!({}),
        })
        .expect("a test delivery serializes");
        let job = Job {
            hook: hook.clone(),
            event: webhook::TEST_EVENT.to_owned(),
            delivery,
            time_unix_ms,
            body: Arc::new(body),
        };
        let once = Policy {
            attempts: 1,
            ..self.policy
        };
        let result = deliver(&self.client, once, &job).await;
        finish(&self.status, &job, result)
    }

    /// The last delivery to `url`.
    pub fn status(&self, url: &str) -> Option<Status> {
        self.status.lock().unwrap().get(url).cloned()
    }

    /// Ends `url`'s task once its queue drains and forgets its status: the
    /// hook was deleted or moved.
    pub fn forget(&self, url: &str) {
        self.queues.lock().unwrap().remove(url);
        self.status.lock().unwrap().remove(url);
    }

    fn record(&self, url: &str, status: Status) {
        self.status.lock().unwrap().insert(url.to_owned(), status);
    }
}

/// How one delivery ended: the attempts it took, the last HTTP status, and
/// the error if it failed.
struct Outcome {
    attempts: u32,
    http_status: Option<u16>,
    error: Option<String>,
}

async fn deliver(client: &reqwest::Client, policy: Policy, job: &Job) -> Outcome {
    let signature = format!(
        "{}{}",
        webhook::SIGNATURE_PREFIX,
        webhook::sign(&job.hook.secret, &job.body)
    );
    let mut outcome = Outcome {
        attempts: 0,
        http_status: None,
        error: None,
    };
    let mut wait = policy.backoff;
    for attempt in 1..=policy.attempts.max(1) {
        if attempt > 1 {
            tokio::time::sleep(wait).await;
            wait = wait.saturating_mul(2);
        }
        outcome.attempts = attempt;
        let sent = client
            .post(&job.hook.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(webhook::HEADER_EVENT, &job.event)
            .header(webhook::HEADER_DELIVERY, &job.delivery)
            .header(webhook::HEADER_TIMESTAMP, job.time_unix_ms.to_string())
            .header(webhook::HEADER_SIGNATURE, &signature)
            .body(job.body.as_ref().clone())
            .timeout(policy.timeout)
            .send()
            .await;
        let retry = match sent {
            Ok(response) => {
                let status = response.status();
                outcome.http_status = Some(status.as_u16());
                if status.is_success() {
                    outcome.error = None;
                    return outcome;
                }
                outcome.error = Some(format!("the receiver answered HTTP {}", status.as_u16()));
                // Another try won't fix a refused signature or a missing page.
                status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
            Err(error) => {
                outcome.http_status = None;
                outcome.error = Some(if error.is_timeout() {
                    format!("no answer within {} ms", policy.timeout.as_millis())
                } else {
                    // Without the URL, which may hold a token.
                    format!("{:#}", anyhow::Error::from(error.without_url()))
                });
                true
            }
        };
        if !retry {
            break;
        }
    }
    outcome
}

fn finish(status: &Mutex<HashMap<String, Status>>, job: &Job, outcome: Outcome) -> Status {
    let ok = outcome.error.is_none();
    if !ok {
        tracing::warn!(
            hook = %origin(&job.hook.url),
            event = %job.event,
            delivery = %job.delivery,
            attempts = outcome.attempts,
            http_status = outcome.http_status,
            error = outcome.error.as_deref().unwrap_or(""),
            "webhook delivery failed"
        );
    }
    let next = Status {
        event: job.event.clone(),
        delivery: job.delivery.clone(),
        time_unix_ms: job.time_unix_ms,
        ok,
        http_status: outcome.http_status,
        error: outcome.error,
        attempts: outcome.attempts,
    };
    status
        .lock()
        .unwrap()
        .insert(job.hook.url.clone(), next.clone());
    next
}

/// A device in an event's `data`.
pub fn device(uuid: &str, name: &str) -> Value {
    serde_json::json!({"uuid": uuid, "name": name})
}

/// `scheme://host:port` of `url`, for logs.
pub fn origin(url: &str) -> String {
    url::Url::parse(url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|_| "<invalid URL>".into())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

impl Host {
    /// `app.started` for an app just installed as the running one, unless it
    /// is a placeholder waiting for a sign-in, which raises it once it starts.
    pub fn app_started_webhook(&self, started: Option<(u32, String, String)>) {
        if let Some((id, name, owner)) = started {
            self.webhook(
                "app.started",
                serde_json::json!({"app": {"id": id, "name": name}, "device": self.webhook_device(&owner)}),
            );
        }
    }

    /// `session.started` or `session.ended` for a stream session.
    pub fn session_webhook(&self, event: &str, s: &crate::state::Session) {
        let launch = &s.launch;
        let mut data = serde_json::json!({
            "session": launch.id,
            "device": device(&launch.client.uuid, &launch.client.name),
            "app_id": launch.app_id,
            "role": match launch.role {
                butterpollo_core::session::Role::Stream => "stream",
                butterpollo_core::session::Role::RemoteMonitor => "remote_monitor",
                butterpollo_core::session::Role::InputOnly => "input_only",
            },
            "transport": match launch.transport {
                butterpollo_core::session::Transport::GameStream => "moonlight",
                butterpollo_core::session::Transport::WebRtc => "webrtc",
            },
        });
        if event == "session.ended" {
            data["failed"] = s.failed().into();
        }
        self.webhook(event, data);
    }

    /// A paired device as events name it, or null for none (the console).
    pub fn webhook_device(&self, uuid: &str) -> Value {
        if uuid.is_empty() {
            return Value::Null;
        }
        let name = self
            .paired
            .read()
            .unwrap()
            .clients
            .iter()
            .find(|c| c.uuid == uuid)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        device(uuid, &name)
    }

    /// Raises `event` for the hooks that subscribe to it. Cheap when none do;
    /// never call it while holding `paired` or `config`.
    pub fn webhook(&self, event: &str, data: Value) {
        debug_assert!(webhook::EVENTS.contains(&event), "{event}");
        let (hooks, name) = {
            let config = self.config.read().unwrap();
            if config.get("webhooks", "[]").trim() == "[]" {
                return;
            }
            (config.webhooks(), crate::network::host_name(&config))
        };
        if hooks.is_empty() {
            return;
        }
        let unique_id = self.paired.read().unwrap().unique_id.clone();
        self.webhooks.emit(
            &hooks,
            DeliveryHost {
                name: &name,
                unique_id: &unique_id,
            },
            event,
            data,
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
    pub(crate) async fn receiver(
        statuses: Vec<u16>,
    ) -> (String, mpsc::UnboundedReceiver<Received>) {
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
        webhooks.emit(&hooks, HOST, "session.started", json!({}));
        let first = received.recv().await.unwrap();
        let second = received.recv().await.unwrap();
        assert_eq!(first.body, second.body);
        assert_eq!(
            first.headers["x-rubylight-signature"],
            second.headers["x-rubylight-signature"]
        );
        let status = settled(&webhooks, &url).await;
        assert!(status.ok, "{status:?}");
        assert_eq!((status.http_status, status.attempts), (Some(200), 2));
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
}
