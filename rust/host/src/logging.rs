//! The host's log filter. It follows the console's log level without a
//! restart; RUST_LOG, when set, stays in charge instead.

use std::sync::OnceLock;
use tracing_subscriber::{EnvFilter, Registry, reload};

static FILTER: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// Debug applies to Rubylight's own crates, so the HTTP, TLS and discovery
/// libraries do not flood a debug log.
fn directives(level: &str) -> &str {
    match level {
        "debug" => "info,butterpollo=debug,butterpollo_core=debug,butterpollo_windows=debug",
        level => level,
    }
}

/// The filter layer for the subscriber, from RUST_LOG or the saved level.
pub fn layer(level: &str) -> reload::Layer<EnvFilter, Registry> {
    let environment = EnvFilter::try_from_default_env();
    let fixed = environment.is_ok();
    let (layer, handle) =
        reload::Layer::new(environment.unwrap_or_else(|_| EnvFilter::new(directives(level))));
    if !fixed {
        let _ = FILTER.set(handle);
    }
    layer
}

/// Applies a newly saved log level to the running host.
pub fn apply(level: &str) {
    if let Some(handle) = FILTER.get()
        && let Err(error) = handle.reload(EnvFilter::new(directives(level)))
    {
        tracing::warn!(%error, "the new log level could not be applied; it applies after a restart");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_stays_on_the_hosts_own_crates() {
        assert_eq!(super::directives("info"), "info");
        assert_eq!(super::directives("trace"), "trace");
        assert!(super::directives("debug").starts_with("info,butterpollo=debug"));
    }
}
