//! Quick Suspend's native API operations for Java (`StreamMenuSuspendSection`): `quick_suspend`
//! reads the state, `quick_suspend.suspend` and `quick_suspend.resume` pause and resume the game.
//! The host's refusal is an answer (`state` `refused` with its `reason`), so it arrives as `ok`.
//! [`super::request`] tries these before the rest of [`super::execute`].

use super::ok;
use nightjar_core::http::native_api::quick_suspend::QuickSuspendStatus;
use nightjar_core::http::{Error, HostClient};

/// The calls of `HostClient` Quick Suspend makes, so the dispatch is testable without a host.
pub trait SuspendHost {
    fn quick_suspend(&self) -> Result<QuickSuspendStatus, Error>;
    fn suspend_game(&self) -> Result<QuickSuspendStatus, Error>;
    fn resume_game(&self) -> Result<QuickSuspendStatus, Error>;
}

impl SuspendHost for HostClient {
    fn quick_suspend(&self) -> Result<QuickSuspendStatus, Error> {
        HostClient::quick_suspend(self)
    }
    fn suspend_game(&self) -> Result<QuickSuspendStatus, Error> {
        HostClient::suspend_game(self)
    }
    fn resume_game(&self) -> Result<QuickSuspendStatus, Error> {
        HostClient::resume_game(self)
    }
}

/// The envelope for a Quick Suspend operation, or None for any other.
pub fn execute(host: &dyn SuspendHost, op: &str) -> Option<String> {
    let answer = match op {
        "quick_suspend" => host.quick_suspend(),
        "quick_suspend.suspend" => host.suspend_game(),
        "quick_suspend.resume" => host.resume_game(),
        _ => return None,
    };
    Some(ok(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const PAUSED: &str = include_str!("../../../native-api/tests/fixtures/quick_suspend.json");
    const REFUSED: &str =
        include_str!("../../../native-api/tests/fixtures/quick_suspend_refused.json");

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn answer(&self, call: &'static str, body: &str) -> Result<QuickSuspendStatus, Error> {
            self.calls.lock().unwrap().push(call);
            Ok(serde_json::from_str(body).unwrap())
        }
    }

    impl SuspendHost for Fake {
        fn quick_suspend(&self) -> Result<QuickSuspendStatus, Error> {
            self.answer("read", PAUSED)
        }
        fn suspend_game(&self) -> Result<QuickSuspendStatus, Error> {
            self.answer("suspend", REFUSED)
        }
        fn resume_game(&self) -> Result<QuickSuspendStatus, Error> {
            Err(Error::Malformed("no answer".into()))
        }
    }

    fn parse(envelope: Option<String>) -> serde_json::Value {
        serde_json::from_str(&envelope.expect("a Quick Suspend operation")).unwrap()
    }

    #[test]
    fn reads_pauses_and_resumes_reach_the_host_and_a_refusal_is_an_answer() {
        let fake = Fake::default();
        let read = parse(execute(&fake, "quick_suspend"));
        assert_eq!(read["ok"]["state"], "suspended");
        assert_eq!(read["ok"]["suspended_by"], "Living room TV");
        let refused = parse(execute(&fake, "quick_suspend.suspend"));
        assert_eq!(refused["ok"]["state"], "refused");
        assert_eq!(refused["ok"]["reason"], "anti_cheat_detected");
        let failed = parse(execute(&fake, "quick_suspend.resume"));
        assert_eq!(failed["error"]["kind"], "malformed");
        assert_eq!(*fake.calls.lock().unwrap(), ["read", "suspend"]);
    }

    #[test]
    fn other_operations_go_on_to_the_rest_of_the_dispatch() {
        let fake = Fake::default();
        for op in ["session", "quick_suspend.pause", "audio", ""] {
            assert_eq!(execute(&fake, op), None, "{op}");
        }
        assert!(fake.calls.lock().unwrap().is_empty());
    }
}
