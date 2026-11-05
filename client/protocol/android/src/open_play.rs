//! "When Nightjar opens" for the app's `OpenPlayModel` (`com.limelight.console.OpenPlayModel`): the
//! decision comes from nightjar-policy, the same code desktop Nightjar asks. One call takes a JSON
//! request and answers JSON (`nightjar_policy::open_play::json`).

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_policy::open_play::json::call(request),
        None => serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_console_OpenPlayModel_call(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let request = native_api::string(&mut env, &request);
    let answer = call(request.as_deref());
    match env.new_string(answer) {
        Ok(answer) => answer.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract with the app: each request exactly as `OpenPlayModel.java` sends it, and
    /// the answer this entry gives. The app's `OpenPlayModelTest` sends and reads the same file, so
    /// a change on either side of the JNI call fails one of the two.
    #[test]
    fn answers_each_request_of_the_shared_contract_exactly() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("open_play_contract.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let request = case["request"].to_string();
            let answer: serde_json::Value = serde_json::from_str(&call(Some(&request))).unwrap();
            assert_eq!(answer, case["response"], "{}", case["name"]);
        }
    }

    #[test]
    fn names_a_missing_request() {
        let answer: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }
}
