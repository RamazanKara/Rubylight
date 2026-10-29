//! Shared trace fixtures in `tests/traces/<module>/*.json`. `cargo test` replays them here and
//! Android's `PolicyTraceTest` replays the same files against the Java originals.
//!
//! A trace is one JSON object:
//! - `rust`: the subject, registered in [`subject`] (for example `recovery::StreamRecoveryPolicy`).
//! - `java`: the Java class it must match, or `null` for desktop-only policy.
//! - `new`: the constructor arguments, or `null` when the trace calls only static functions.
//! - `calls`: steps in order. `{"new": [...]}` rebuilds the subject, as a replaced Android
//!   activity does from its saved state. `{"call": "snake_name", "args": [...], "expect": v}`
//!   calls a method; `expect` is `null` when it returns nothing. Java calls the camelCase name.
//!
//! Absent times and delays use Java's -1.

use serde_json::Value;

/// A policy driven by a trace. Each module adapts its own types next to them.
pub trait Subject {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String>;
}

fn subject(name: &str, args: &Value) -> Result<Box<dyn Subject>, String> {
    match name {
        "recovery::StreamRecoveryPolicy" => crate::recovery::traced_policy(args),
        "recovery::Backoff" => crate::recovery::traced_backoff(args),
        "bitrate::AdaptiveBitrateController" => crate::bitrate::traced_controller(args),
        "client_hint::Bitrate" => crate::client_hint::traced(args),
        "quality::AdaptiveQualityController" => crate::quality::traced_controller(args),
        "pyrowave::BitrateController" => crate::pyrowave::traced_controller(args),
        "pyrowave::BitrateFloor" => crate::pyrowave::traced_floor(args),
        "health::StreamHealth" => crate::health::traced_health(args),
        "health::PeriodicStalls" => crate::health::traced_stalls(args),
        "health::BitrateTrial" => crate::health::traced_trial(args),
        "stats::LateFrames" => crate::stats::traced_late(args),
        "launch::LaunchPolicy" => crate::launch::traced(crate::launch::Traced::Launch, args),
        "launch::StreamExitPolicy" => crate::launch::traced(crate::launch::Traced::Exit, args),
        "launch::InputOnlyPolicy" => crate::launch::traced(crate::launch::Traced::InputOnly, args),
        _ => Err(format!("unknown subject {name}")),
    }
}

/// Replays one trace and returns how many steps it ran.
pub fn replay(trace: &Value) -> Result<usize, String> {
    let name = trace["rust"].as_str().ok_or("no rust subject")?;
    if trace.get("java").is_none() {
        return Err("no java class (use null for desktop-only policy)".into());
    }
    let steps = trace["calls"].as_array().ok_or("no calls")?;
    if steps.is_empty() {
        return Err("no calls".into());
    }
    let mut current = subject(name, &trace["new"])?;
    for (i, step) in steps.iter().enumerate() {
        if let Some(args) = step.get("new") {
            current = subject(name, args).map_err(|e| format!("step {i}: {e}"))?;
            continue;
        }
        let method = step["call"]
            .as_str()
            .ok_or_else(|| format!("step {i}: no call"))?;
        let expect = step
            .get("expect")
            .ok_or_else(|| format!("step {i}: no expect"))?;
        let args = &step["args"];
        let actual = current
            .call(method, args)
            .map_err(|e| format!("step {i} {method}{args}: {e}"))?;
        if !same(&actual, expect) {
            return Err(format!(
                "step {i} {method}{args}: expected {expect}, got {actual}"
            ));
        }
    }
    Ok(steps.len())
}

fn same(actual: &Value, expect: &Value) -> bool {
    match (actual, expect) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
            a.as_f64() == b.as_f64()
        }
        _ => actual == expect,
    }
}

/// A call's arguments, checked for count and type.
pub struct Args<'a>(&'a [Value]);

impl<'a> Args<'a> {
    pub fn new(args: &'a Value, count: usize) -> Result<Self, String> {
        let values = args
            .as_array()
            .ok_or_else(|| format!("arguments are not a list: {args}"))?;
        if values.len() != count {
            return Err(format!("expected {count} arguments, got {}", values.len()));
        }
        Ok(Self(values))
    }

    pub fn int<T: TryFrom<i64>>(&self, i: usize) -> Result<T, String> {
        self.0[i]
            .as_i64()
            .and_then(|v| T::try_from(v).ok())
            .ok_or_else(|| {
                let kind = std::any::type_name::<T>();
                format!("argument {i} is not a {kind}: {}", self.0[i])
            })
    }

    /// A time that may be absent (-1).
    pub fn optional_ms(&self, i: usize) -> Result<Option<u64>, String> {
        match self.int::<i64>(i)? {
            -1 => Ok(None),
            _ => self.int(i).map(Some),
        }
    }

    pub fn text(&self, i: usize) -> Result<Option<&'a str>, String> {
        match &self.0[i] {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text)),
            other => Err(format!("argument {i} is not a string: {other}")),
        }
    }
}

/// A delay or time that may be absent, as Java returns it (-1).
pub fn optional_ms(value: Option<u64>) -> Value {
    value.map_or(Value::from(-1), Value::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trace(calls: Value) -> Value {
        json!({"rust": "recovery::StreamRecoveryPolicy", "java": null, "new": [0, 0], "calls": calls})
    }

    #[test]
    fn a_wrong_expectation_fails() {
        let call = |expect: i64| json!([{"call": "next_delay", "args": [5, -1], "expect": expect}]);
        assert_eq!(replay(&trace(call(1000))), Ok(1));
        let error = replay(&trace(call(2000))).unwrap_err();
        assert!(error.contains("expected 2000, got 1000"), "{error}");
    }

    #[test]
    fn malformed_traces_fail() {
        for calls in [
            json!([]),
            json!([{"call": "next_delay", "args": [5, -1]}]),
            json!([{"call": "next_delay", "args": [5], "expect": 1000}]),
            json!([{"call": "next_delay", "args": [-5, -1], "expect": 1000}]),
            json!([{"call": "no_such_method", "args": [], "expect": null}]),
        ] {
            assert!(replay(&trace(calls.clone())).is_err(), "{calls}");
        }
        let mut unknown = trace(json!([{"call": "attempts", "args": [], "expect": 0}]));
        assert!(replay(&unknown).is_ok());
        unknown["rust"] = json!("recovery::Nothing");
        assert!(replay(&unknown).is_err());
        unknown.as_object_mut().unwrap().remove("java");
        unknown["rust"] = json!("recovery::StreamRecoveryPolicy");
        assert!(replay(&unknown).is_err());
    }

    #[test]
    fn numbers_compare_by_value() {
        assert!(same(&json!(1.0), &json!(1)));
        assert!(same(&json!(-1), &Value::from(-1i64)));
        assert!(!same(&json!(1.5), &json!(1)));
        assert!(!same(&json!(1), &json!(true)));
    }
}
