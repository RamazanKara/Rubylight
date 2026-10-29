//! Capturing what a test logs, deterministically.
//!
//! `tracing` caches, per call site, whether any subscriber wants its events.
//! While only one subscriber is alive, a thread that reaches a call site first
//! computes that interest from its own thread's subscriber alone (tracing-core's
//! `JustOne` path). The tests run in parallel: a test without a subscriber that
//! reaches `tracing::warn!` before the capturing test does caches "never" for
//! the call site, and the capturing test's event is dropped before it gets to
//! its subscriber. Tests would then read an empty log, now and then.
//!
//! A process-wide subscriber that answers "sometimes" for every call site rules
//! that out: the cache never says "never" for a call site, so each event asks the
//! emitting thread's own subscriber, which is `enabled`'s answer for that thread.

use std::sync::Once;
use tracing::{
    Event, Metadata,
    span::{Attributes, Id, Record},
    subscriber::{DefaultGuard, Interest, Subscriber},
};

/// `S`, except that every call site is cached as "sometimes", so that a thread's
/// own subscriber decides about each event.
struct AskEachTime<S>(S);

impl<S: Subscriber> Subscriber for AskEachTime<S> {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        self.0.max_level_hint()
    }
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.0.enabled(metadata)
    }
    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        self.0.new_span(attributes)
    }
    fn record(&self, span: &Id, values: &Record<'_>) {
        self.0.record(span, values)
    }
    fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.0.record_follows_from(span, follows)
    }
    fn event_enabled(&self, event: &Event<'_>) -> bool {
        self.0.event_enabled(event)
    }
    fn event(&self, event: &Event<'_>) {
        self.0.event(event)
    }
    fn enter(&self, span: &Id) {
        self.0.enter(span)
    }
    fn exit(&self, span: &Id) {
        self.0.exit(span)
    }
    fn clone_span(&self, id: &Id) -> Id {
        self.0.clone_span(id)
    }
    fn try_close(&self, id: Id) -> bool {
        self.0.try_close(id)
    }
}

/// The process-wide subscriber, once: what a failed test's host logged goes to
/// the test writer, which libtest captures per test.
pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(AskEachTime(
            tracing_subscriber::fmt()
                .with_test_writer()
                .with_max_level(tracing::Level::INFO)
                .finish(),
        ));
    });
}

/// `subscriber` is this thread's until the guard drops.
pub(crate) fn capture(subscriber: impl Subscriber + Send + Sync + 'static) -> DefaultGuard {
    install();
    tracing::subscriber::set_default(subscriber)
}

/// Runs `run` with `subscriber` as this thread's.
pub(crate) fn capture_during<T>(
    subscriber: impl Subscriber + Send + Sync + 'static,
    run: impl FnOnce() -> T,
) -> T {
    let _guard = capture(subscriber);
    run()
}
