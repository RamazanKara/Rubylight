//! Sends the events of the `webhooks` setting ([`butterpollo_core::webhook`]).
//!
//! Nothing here runs on a stream's path: an event is serialized once and put
//! on its hook's bounded queue with `try_send`, so a full queue drops the
//! delivery (logged, and shown in the console) instead of waiting. Each hook
//! has its own task, so a slow receiver holds up only its own deliveries,
//! which arrive in order. A delivery is tried [`Policy::attempts`] times, each
//! within [`Policy::timeout`], waiting [`Policy::backoff`] doubled between
//! tries. Each try is stamped with its own time and signed again, keeping the
//! delivery id a receiver de-duplicates by, so a delivery that waited behind a
//! dead receiver still falls in its replay window; one raised longer than
//! [`Policy::max_age`] ([`webhook::REPLAY_WINDOW_MS`]) ago is dropped instead.
//! A failure is logged with the hook's origin only, since a URL's path or
//! query may hold a token, and the secret is never logged.
use crate::state::Host;
use butterpollo_core::webhook::{self, Delivery, DeliveryHost, Webhook};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Whether `ip` is this PC or link-local: services there (the host's own
/// API, a router's or a cloud's metadata page) may trust whoever can reach
/// them, so no hook may send there. LAN addresses stay allowed.
pub(crate) fn on_this_pc_or_link(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_link_local() || ip.is_unspecified(),
        std::net::IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => ip.is_loopback() || ip.is_link_local() || ip.is_unspecified(),
            None => ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unspecified(),
        },
    }
}

/// Resolves a hook's host name at connect time and drops the addresses
/// [`on_this_pc_or_link`] refuses. The console checks only the saved text,
/// and a name can point (or later rebind) to 127.0.0.1 or 169.254.169.254.
struct Resolver;
impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let name = name.as_str().to_owned();
            let resolved: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let allowed = allowed_addresses(resolved);
            if allowed.is_empty() {
                return Err(format!(
                    "{name} resolves only to this PC or link-local addresses, which webhooks may not reach"
                )
                .into());
            }
            Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
        })
    }
}
fn allowed_addresses(
    resolved: impl IntoIterator<Item = std::net::SocketAddr>,
) -> Vec<std::net::SocketAddr> {
    resolved
        .into_iter()
        .filter(|address| !on_this_pc_or_link(address.ip()))
        .collect()
}

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
    /// How long after its event a delivery may still be tried; an older one
    /// is dropped, as a receiver would refuse it as a replay.
    pub max_age: Duration,
}

impl Policy {
    /// At most 4 tries over about 27 s, each stamped anew; a delivery still
    /// queued [`webhook::REPLAY_WINDOW_MS`] after its event is dropped.
    pub const DEFAULT: Self = Self {
        timeout: Duration::from_secs(5),
        attempts: 4,
        backoff: Duration::from_secs(1),
        max_age: Duration::from_millis(webhook::REPLAY_WINDOW_MS),
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

/// One delivery to one hook. Its body is written at each try, with that
/// try's time.
#[derive(Clone)]
struct Job {
    hook: Webhook,
    event: String,
    delivery: String,
    /// When the event happened, for the console.
    time_unix_ms: u64,
    /// When the event happened, for [`Policy::max_age`].
    raised: Instant,
    host_name: Arc<str>,
    host_id: Arc<str>,
    data: Arc<Value>,
}

impl Job {
    fn new(hook: &Webhook, host: DeliveryHost<'_>, event: &str, data: Value) -> Self {
        Self {
            hook: hook.clone(),
            event: event.to_owned(),
            delivery: uuid::Uuid::new_v4().to_string(),
            time_unix_ms: now_ms(),
            raised: Instant::now(),
            host_name: host.name.into(),
            host_id: host.unique_id.into(),
            data: Arc::new(data),
        }
    }

    /// The body, stamped `time_unix_ms`.
    fn body(&self, time_unix_ms: u64) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(&Delivery {
            event: &self.event,
            delivery: &self.delivery,
            time_unix_ms,
            host: DeliveryHost {
                name: &self.host_name,
                unique_id: &self.host_id,
            },
            data: self.data.as_ref().clone(),
        })
    }
}

/// A hook's queue, and the flag [`Webhooks::forget`] sets to drop what is
/// still in it.
struct Queue {
    sender: mpsc::Sender<Job>,
    forgotten: Arc<AtomicBool>,
}

pub struct Webhooks {
    client: reqwest::Client,
    policy: Policy,
    /// The runtime of the thread that made this, for events raised on
    /// threads outside it.
    runtime: Option<tokio::runtime::Handle>,
    /// Each hook's queue, by URL.
    queues: Mutex<HashMap<String, Queue>>,
    status: Arc<Mutex<HashMap<String, Status>>>,
}

impl Webhooks {
    pub fn new(policy: Policy) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            // A redirect could send the signed body somewhere else.
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(Resolver))
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
        // One delivery id and event time for every hook.
        let first = Job::new(hooks[0], host, event, data);
        let (delivery, time_unix_ms) = (first.delivery.clone(), first.time_unix_ms);
        let mut queued = 0;
        for hook in hooks {
            let job = Job {
                hook: hook.clone(),
                ..first.clone()
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
            Some(queue) => match queue.sender.try_send(job) {
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
        let forgotten = Arc::new(AtomicBool::new(false));
        let dropped = forgotten.clone();
        runtime.spawn(async move {
            while let Some(job) = receiver.recv().await {
                let Some(result) = deliver(&client, policy, &job, &dropped).await else {
                    // Ending drops the rest of the queue too.
                    tracing::info!(
                        hook = %origin(&job.hook.url),
                        "the webhook was deleted or moved: its queued deliveries were dropped"
                    );
                    break;
                };
                finish(&status, &job, result, &dropped);
            }
        });
        let url = job.hook.url.clone();
        sender
            .try_send(job)
            .map_err(|_| "the hook's queue is full")?;
        queues.insert(url, Queue { sender, forgotten });
        Ok(())
    }

    /// Sends [`webhook::TEST_EVENT`] to `hook` once, now, and returns how it went.
    pub async fn test(&self, hook: &Webhook, host: DeliveryHost<'_>) -> Status {
        let job = Job::new(hook, host, webhook::TEST_EVENT, serde_json::json!({}));
        let once = Policy {
            attempts: 1,
            ..self.policy
        };
        let kept = AtomicBool::new(false);
        let result = deliver(&self.client, once, &job, &kept)
            .await
            .expect("a test delivery's hook is never forgotten");
        finish(&self.status, &job, result, &kept)
    }

    /// The last delivery to `url`.
    pub fn status(&self, url: &str) -> Option<Status> {
        self.status.lock().unwrap().get(url).cloned()
    }

    /// Drops `url`'s queued deliveries, ends its task (before the next try of
    /// one it is sending) and forgets its status: the hook was deleted or
    /// moved, so they would go where it no longer points.
    pub fn forget(&self, url: &str) {
        if let Some(queue) = self.queues.lock().unwrap().remove(url) {
            // Before the status goes: a delivery that finishes meanwhile records nothing.
            queue.forgotten.store(true, Ordering::SeqCst);
        }
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

/// Tries `job` as `policy` says; None when its hook was `forgotten`
/// ([`Webhooks::forget`]) before a try.
async fn deliver(
    client: &reqwest::Client,
    policy: Policy,
    job: &Job,
    forgotten: &AtomicBool,
) -> Option<Outcome> {
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
        if forgotten.load(Ordering::SeqCst) {
            return None;
        }
        // A receiver following docs/webhooks.md would refuse it as a replay.
        if job.raised.elapsed() > policy.max_age {
            outcome.http_status = None;
            outcome.error = Some(format!(
                "dropped after waiting longer than the {} s replay window",
                policy.max_age.as_secs()
            ));
            break;
        }
        // Each try has its own time and signature, and the same delivery id.
        let time_unix_ms = now_ms();
        let body = match job.body(time_unix_ms) {
            Ok(body) => body,
            Err(error) => {
                outcome.error = Some(format!("the body could not be written: {error}"));
                break;
            }
        };
        let signature = format!(
            "{}{}",
            webhook::SIGNATURE_PREFIX,
            webhook::sign(&job.hook.secret, &body)
        );
        outcome.attempts = attempt;
        let sent = client
            .post(&job.hook.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(webhook::HEADER_EVENT, &job.event)
            .header(webhook::HEADER_DELIVERY, &job.delivery)
            .header(webhook::HEADER_TIMESTAMP, time_unix_ms.to_string())
            .header(webhook::HEADER_SIGNATURE, &signature)
            .body(body)
            .timeout(policy.timeout)
            .send()
            .await;
        let retry = match sent {
            Ok(response) => {
                let status = response.status();
                outcome.http_status = Some(status.as_u16());
                if status.is_success() {
                    outcome.error = None;
                    return Some(outcome);
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
    Some(outcome)
}

/// Logs and records how `job` went; not recorded once its hook is
/// `forgotten`, whose status is gone.
fn finish(
    status: &Mutex<HashMap<String, Status>>,
    job: &Job,
    outcome: Outcome,
    forgotten: &AtomicBool,
) -> Status {
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
    let mut status = status.lock().unwrap();
    // Read under the lock forget() takes after setting it.
    if !forgotten.load(Ordering::SeqCst) {
        status.insert(job.hook.url.clone(), next.clone());
    }
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
            .find(|c| c.uuid == butterpollo_core::state::device_uuid(uuid))
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
pub(crate) mod tests;
