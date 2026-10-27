//! `GET /bandwidth-probe?bytes=N`: a paired client times an incompressible
//! body to measure the path before it picks a bitrate. Defined for 3.0; the
//! host serves the route only once it offers `BandwidthProbeVersion`.
//!
//! The host answers `application/octet-stream` with `Cache-Control: no-store`
//! and stops writing after [`MAX_DURATION`]. A bad `bytes` is 400, a stream
//! running or starting is 409, and a probe within [`COOLDOWN`] of the client's
//! last one, or past [`BUDGET_BYTES`] in [`BUDGET_WINDOW`], is 429 with
//! `Retry-After`.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// The route, on the paired HTTPS port.
pub const ROUTE: &str = "/bandwidth-probe";
/// Serverinfo element: the host serves [`ROUTE`].
pub const SERVERINFO_FIELD: &str = "BandwidthProbeVersion";
/// Smallest and largest body a client may ask for.
pub const MIN_BYTES: u32 = 64 * 1024;
pub const MAX_BYTES: u32 = 4 * 1024 * 1024;
/// The host stops writing a body after this long.
pub const MAX_DURATION: Duration = Duration::from_secs(3);
/// Shortest gap between two probes of one client.
pub const COOLDOWN: Duration = Duration::from_secs(5);
/// Bytes one client may probe within [`BUDGET_WINDOW`].
pub const BUDGET_BYTES: u64 = 16 << 20;
pub const BUDGET_WINDOW: Duration = Duration::from_secs(60);

/// The `bytes` query value: decimal digits within [`MIN_BYTES`]..=[`MAX_BYTES`].
pub fn parse(bytes: Option<&str>) -> Result<u32> {
    let Some(text) = bytes else {
        bail!("bytes is required");
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        bail!("invalid bytes {text:?}");
    }
    let bytes: u32 = text
        .parse()
        .with_context(|| format!("invalid bytes {text:?}"))?;
    if !(MIN_BYTES..=MAX_BYTES).contains(&bytes) {
        bail!("bytes must be {MIN_BYTES} to {MAX_BYTES}");
    }
    Ok(bytes)
}

/// Each client's probes: [`COOLDOWN`] between two, [`BUDGET_BYTES`] per
/// [`BUDGET_WINDOW`].
#[derive(Debug, Default)]
pub struct Limiter {
    clients: HashMap<String, Vec<(Instant, u64)>>,
}

impl Limiter {
    /// Admits a probe of `bytes` for `client`, or says how long until one fits.
    pub fn admit(&mut self, client: &str, bytes: u32, now: Instant) -> Result<(), Duration> {
        // Forget what left the window, for every client: the map stays small.
        self.clients.retain(|_, probes| {
            probes.retain(|(at, _)| now.saturating_duration_since(*at) < BUDGET_WINDOW);
            !probes.is_empty()
        });
        let probes = self.clients.entry(client.to_owned()).or_default();
        if let Some((last, _)) = probes.last() {
            let since = now.saturating_duration_since(*last);
            if since < COOLDOWN {
                return Err(COOLDOWN - since);
            }
        }
        let mut used: u64 = probes.iter().map(|(_, bytes)| bytes).sum();
        let mut retry = Duration::ZERO;
        for (at, spent) in probes.iter() {
            if used + u64::from(bytes) <= BUDGET_BYTES {
                break;
            }
            used -= spent;
            retry = BUDGET_WINDOW.saturating_sub(now.saturating_duration_since(*at));
        }
        if !retry.is_zero() {
            return Err(retry);
        }
        probes.push((now, u64::from(bytes)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_waits_out_the_cooldown_and_the_budget() {
        let start = Instant::now();
        let mut limiter = Limiter::default();
        assert_eq!(limiter.admit("a", MAX_BYTES, start), Ok(()));
        let soon = start + Duration::from_secs(2);
        assert_eq!(
            limiter.admit("a", MIN_BYTES, soon),
            Err(Duration::from_secs(3))
        );
        // Another client has its own cooldown and budget.
        assert_eq!(limiter.admit("b", MAX_BYTES, soon), Ok(()));
        let mut now = start;
        for _ in 0..3 {
            now += COOLDOWN;
            assert_eq!(limiter.admit("a", MAX_BYTES, now), Ok(()));
        }
        // 16 MB in the window: the next probe fits once the first leaves it.
        now += COOLDOWN;
        assert_eq!(
            limiter.admit("a", MIN_BYTES, now),
            Err(BUDGET_WINDOW - Duration::from_secs(20))
        );
        assert_eq!(limiter.admit("a", MIN_BYTES, start + BUDGET_WINDOW), Ok(()));
        // A refused probe doesn't count.
        assert_eq!(
            limiter.admit("b", MIN_BYTES, start + BUDGET_WINDOW + COOLDOWN),
            Ok(())
        );
    }

    #[test]
    fn the_limits_are_the_spec() {
        assert_eq!((MIN_BYTES, MAX_BYTES), (65_536, 4_194_304));
        assert_eq!(BUDGET_BYTES, 16 * 1024 * 1024);
        assert_eq!(
            (MAX_DURATION, COOLDOWN, BUDGET_WINDOW),
            (
                Duration::from_secs(3),
                Duration::from_secs(5),
                Duration::from_secs(60)
            )
        );
    }

    #[test]
    fn bytes_are_bounded_digits() {
        assert_eq!(parse(Some("65536")).unwrap(), MIN_BYTES);
        assert_eq!(parse(Some("4194304")).unwrap(), MAX_BYTES);
        assert_eq!(parse(Some("1000000")).unwrap(), 1_000_000);
        for bad in [
            None,
            Some(""),
            Some("65535"),
            Some("4194305"),
            Some("99999999999"),
            Some("+65536"),
            Some("-1"),
            Some("1e6"),
            Some(" 65536"),
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
