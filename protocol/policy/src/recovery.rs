//! Automatic reconnecting: which failures may retry, and the retry budget.
//!
//! [`StreamRecoveryPolicy`] is Android's (`StreamRecoveryPolicy.java`). [`Backoff`] is the
//! desktop client's budget, which starts its window at the drop and resets on a stable
//! connection's length instead of its start time.

use serde_json::Value;

use crate::trace::{self, Args, Subject};

pub const MAX_ATTEMPTS: u32 = 5;
pub const WINDOW_MS: u64 = 60_000;
pub const STABLE_MS: u64 = 30_000;
const DELAYS_MS: [u64; MAX_ATTEMPTS as usize] = [1000, 2000, 4000, 8000, 12000];

/// A retry budget that survives replacing the stream activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamRecoveryPolicy {
    attempts: u32,
    started_at_ms: u64,
}

impl StreamRecoveryPolicy {
    /// Restores saved state; an out-of-range attempt count is clamped.
    pub fn new(attempts: u32, started_at_ms: u64) -> Self {
        Self {
            attempts: attempts.min(MAX_ATTEMPTS),
            started_at_ms,
        }
    }

    /// The delay before the next attempt, or `None` when the budget is spent.
    pub fn next_delay(&mut self, now_ms: u64, connected_at_ms: Option<u64>) -> Option<u64> {
        self.reset_after_stable_connection(now_ms, connected_at_ms);
        if self.attempts == 0 {
            self.started_at_ms = now_ms;
        }
        if self.attempts >= MAX_ATTEMPTS || now_ms < self.started_at_ms {
            return None;
        }
        let delay = DELAYS_MS[self.attempts as usize];
        if now_ms - self.started_at_ms + delay >= WINDOW_MS {
            return None;
        }
        self.attempts += 1;
        Some(delay)
    }

    /// Whether [`StreamRecoveryPolicy::next_delay`] would allow an attempt now, spending none.
    pub fn allows_attempt(&self, now_ms: u64, connected_at_ms: Option<u64>) -> bool {
        let mut probe = *self;
        probe.next_delay(now_ms, connected_at_ms).is_some()
    }

    pub fn reset_after_stable_connection(&mut self, now_ms: u64, connected_at_ms: Option<u64>) {
        // Briefly starting a stream must not give a flapping connection an unlimited budget.
        if connected_at_ms.is_some_and(|at| now_ms.saturating_sub(at) >= STABLE_MS) {
            *self = Self::default();
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }
}

/// Native media timeouts and transport errors. Graceful (0), protected content (-103),
/// conversion (-104) and unrecognized host errors never retry.
pub fn transient_termination(code: i32) -> bool {
    matches!(code, -102..=-100) || transport_termination(code)
}

/// Whether a stream that ended with `code` reconnects on its own: a [`transient_termination`]
/// of a connection whose video started (its first frame arrived). One that never had video
/// failed to start, most often at a firewall blocking the media ports: reconnecting would only
/// hide why behind up to [`MAX_ATTEMPTS`] attempts, a minute in all. Ported from Moonlight V+
/// PC (qiin2333/moonlight-qt 6792aeed, "avoid reconnect loop before first video").
pub fn recovers_termination(code: i32, video_started: bool) -> bool {
    video_started && transient_termination(code)
}

/// Only ambiguous transport failures can stand in for a lost host-exit packet. Media timeouts
/// and an early host exit keep their meaning. -1 is an ENet failure without an errno.
pub fn transport_termination(code: i32) -> bool {
    matches!(code, -1 | 32 | 54 | 57 | 60 | 64 | 100..=104 | 107 | 110..=113
        | 10050..=10054 | 10057 | 10060 | 10061 | 10064 | 10065)
}

/// Whether a failed start may retry while recovering a previously connected session. Host
/// rejection, pairing, permission and decoder errors need a user decision.
pub fn transient_startup(stage: Option<&str>, port_flags: i32, error: i32) -> bool {
    // NvConnection reports Java transport failures with both HTTP port flags and error 0; its
    // stage is the game's name then, not a native startup stage.
    if port_flags == 0x0001 | 0x0002 && error == 0 {
        return true;
    }
    // Native RTSP and control startup report positive errno values; positive host status codes
    // stay manual, so only known transport errors pass.
    port_flags != 0
        && (error <= 0 || transient_termination(error))
        && stage.is_some_and(|stage| !stage.to_lowercase().contains("video"))
}

/// Between two route checks while the PC is not reachable (Android's `NetworkReturnRecovery`).
pub const ROUTE_RETRY_MS: u64 = 500;
/// One route check's connect timeout: below [`ROUTE_RETRY_MS`], so checks keep their pace.
pub const ROUTE_PROBE_TIMEOUT_MS: u64 = 400;

/// The connection's own timeout without a network drop tolerance (the engine's
/// `CONTROL_PEER_TIMEOUT`): a tolerance up to it changes nothing.
pub const PEER_TIMEOUT_MS: u32 = 10_000;

/// A network is back after the stream's was lost. True: reconnect now, since the engine said
/// the connection cannot carry on over it (`cannot_continue`: the route toward the PC leaves
/// from another address and the stream did not roam there) and would only time out. Never when
/// it roamed or carries on (the same address back, or no route yet: its own timeout decides),
/// with automatic reconnection off, on a `metered` network without "Continue on mobile data"
/// (`mobile_allowed`), or with a network drop tolerance above [`PEER_TIMEOUT_MS`] (it holds
/// the connection, and decides).
pub fn reconnects_at_once(
    cannot_continue: bool,
    metered: bool,
    mobile_allowed: bool,
    auto_reconnect: bool,
    network_grace_ms: u32,
) -> bool {
    cannot_continue
        && (!metered || mobile_allowed)
        && auto_reconnect
        && network_grace_ms <= PEER_TIMEOUT_MS
}

/// The wait for a route to the PC before a recovery attempt. A Wi-Fi network is often back
/// before it has an address toward the PC (DHCP after the reconnect), so the attempt waits,
/// checking every [`ROUTE_RETRY_MS`], instead of failing at once and backing off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteWait {
    deadline_ms: u64,
    unreachable: u32,
}

impl RouteWait {
    /// `deadline_ms`: when the recovery gives up (its window ends).
    pub fn new(deadline_ms: u64) -> Self {
        Self { deadline_ms, unreachable: 0 }
    }

    /// After a route check at `now_ms`: `Some(0)` to start the attempt, `Some(delay)` before the
    /// next check, `None` when the window ended without a route (the recovery gives up, loudly).
    pub fn next(&mut self, now_ms: u64, reachable: bool) -> Option<u64> {
        if reachable {
            return Some(0);
        }
        self.unreachable += 1;
        if now_ms >= self.deadline_ms {
            return None;
        }
        Some(ROUTE_RETRY_MS.min(self.deadline_ms - now_ms))
    }

    /// Route checks that found the PC unreachable so far.
    pub fn unreachable_checks(&self) -> u32 {
        self.unreachable
    }
}

/// The desktop client's reconnect budget: the same delays within one minute of the first drop,
/// renewed only after a connection that lasted [`STABLE_MS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backoff {
    attempts: u32,
    started_at_ms: Option<u64>,
    /// The next attempt starts without its delay (a network came back); it still spends one.
    now: bool,
}

impl Backoff {
    /// Starts the window when the stream drops, before the first attempt.
    pub fn begin(&mut self, now_ms: u64) {
        self.started_at_ms.get_or_insert(now_ms);
    }

    pub fn reset_after(&mut self, connected_ms: u64) {
        if connected_ms >= STABLE_MS {
            *self = Self::default();
        }
    }

    pub fn remaining(&self, now_ms: u64) -> u64 {
        self.started_at_ms.map_or(WINDOW_MS, |started| {
            WINDOW_MS.saturating_sub(now_ms.saturating_sub(started))
        })
    }

    pub fn next_delay(&mut self, now_ms: u64) -> Option<u64> {
        let delay = *DELAYS_MS.get(self.attempts as usize)?;
        let delay = if self.now { 0 } else { delay };
        self.started_at_ms.get_or_insert(now_ms);
        if delay >= self.remaining(now_ms) {
            return None;
        }
        self.now = false;
        self.attempts += 1;
        Some(delay)
    }

    /// Whether [`Backoff::next_delay`] at `now_ms` would allow an attempt, spending none.
    pub fn allows_attempt(&self, now_ms: u64) -> bool {
        DELAYS_MS
            .get(self.attempts as usize)
            .is_some_and(|delay| *delay < self.remaining(now_ms))
    }

    /// The next attempt starts at once: the network came back without the stream roaming.
    pub fn retry_now(&mut self) {
        self.now = true;
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

fn classify(method: &str, args: &Value) -> Option<Result<Value, String>> {
    let result = match method {
        "transient_termination" => Args::new(args, 1)
            .and_then(|a| a.int(0))
            .map(|code| transient_termination(code).into()),
        "recovers_termination" => Args::new(args, 2)
            .and_then(|a| Ok(recovers_termination(a.int(0)?, a.flag(1)?).into())),
        "transport_termination" => Args::new(args, 1)
            .and_then(|a| a.int(0))
            .map(|code| transport_termination(code).into()),
        "transient_startup" => Args::new(args, 3)
            .and_then(|a| Ok(transient_startup(a.text(0)?, a.int(1)?, a.int(2)?).into())),
        _ => return None,
    };
    Some(result)
}

struct TracedPolicy(Option<StreamRecoveryPolicy>);

pub(crate) fn traced_policy(args: &Value) -> Result<Box<dyn Subject>, String> {
    let policy = match args {
        Value::Null => None,
        _ => {
            let a = Args::new(args, 2)?;
            Some(StreamRecoveryPolicy::new(a.int(0)?, a.int(1)?))
        }
    };
    Ok(Box::new(TracedPolicy(policy)))
}

impl Subject for TracedPolicy {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if let Some(result) = classify(method, args) {
            return result;
        }
        let policy = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "next_delay" => {
                let a = Args::new(args, 2)?;
                let delay = policy.next_delay(a.int(0)?, a.optional_ms(1)?);
                Ok(trace::optional_ms(delay))
            }
            "reset_after_stable_connection" => {
                let a = Args::new(args, 2)?;
                policy.reset_after_stable_connection(a.int(0)?, a.optional_ms(1)?);
                Ok(Value::Null)
            }
            "allows_attempt" => {
                let a = Args::new(args, 2)?;
                Ok(policy.allows_attempt(a.int(0)?, a.optional_ms(1)?).into())
            }
            "attempts" => Args::new(args, 0).map(|_| policy.attempts().into()),
            "started_at" => Args::new(args, 0).map(|_| policy.started_at_ms().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}

pub(crate) fn traced_backoff(args: &Value) -> Result<Box<dyn Subject>, String> {
    Args::new(args, 0)?;
    Ok(Box::new(Backoff::default()))
}

impl Subject for Backoff {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "begin" => {
                self.begin(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "reset_after" => {
                self.reset_after(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "remaining" => Ok(self.remaining(Args::new(args, 1)?.int(0)?).into()),
            "next_delay" => {
                let delay = self.next_delay(Args::new(args, 1)?.int(0)?);
                Ok(trace::optional_ms(delay))
            }
            "attempts" => Args::new(args, 0).map(|_| self.attempts().into()),
            "allows_attempt" => Args::new(args, 1).and_then(|a| Ok(self.allows_attempt(a.int(0)?).into())),
            "retry_now" => {
                Args::new(args, 0)?;
                self.retry_now();
                Ok(Value::Null)
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

/// Java's `NetworkReturnRecovery`: `new` is the deadline, or null for the static decision.
pub(crate) fn traced_route_wait(args: &Value) -> Result<Box<dyn Subject>, String> {
    let wait = match args {
        Value::Null => None,
        _ => Some(RouteWait::new(Args::new(args, 1)?.int(0)?)),
    };
    Ok(Box::new(TracedRouteWait(wait)))
}

struct TracedRouteWait(Option<RouteWait>);

impl Subject for TracedRouteWait {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if method == "reconnects_at_once" {
            let a = Args::new(args, 5)?;
            return Ok(reconnects_at_once(a.flag(0)?, a.flag(1)?, a.flag(2)?, a.flag(3)?, a.int(4)?).into());
        }
        let wait = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "next" => {
                let a = Args::new(args, 2)?;
                Ok(trace::optional_ms(wait.next(a.int(0)?, a.flag(1)?)))
            }
            "unreachable_checks" => Args::new(args, 0).map(|_| wait.unreachable_checks().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}
