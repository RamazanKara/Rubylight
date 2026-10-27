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

/// The desktop client's reconnect budget: the same delays within one minute of the first drop,
/// renewed only after a connection that lasted [`STABLE_MS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backoff {
    attempts: u32,
    started_at_ms: Option<u64>,
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
        self.started_at_ms.get_or_insert(now_ms);
        if delay >= self.remaining(now_ms) {
            return None;
        }
        self.attempts += 1;
        Some(delay)
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
            _ => Err(format!("unknown method {method}")),
        }
    }
}
