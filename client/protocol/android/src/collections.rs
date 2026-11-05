//! Home's groups by collection for the app's `com.limelight.console.CollectionGroups`, from the same
//! code desktop Nightjar groups with (`nightjar_policy::collections`).

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_policy::collections::json::call(request),
        None => serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_console_CollectionGroups_call(mut env: JNIEnv, _: JClass, request: JString) -> jstring {
    let request = native_api::string(&mut env, &request);
    match env.new_string(call(request.as_deref())) {
        Ok(answer) => answer.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract with the app: the app's `CollectionGroupsTest` sends and reads the same file, so
    /// a change on either side of the JNI call fails one of the two.
    #[test]
    fn answers_each_request_of_the_shared_contract_exactly() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("collections_contract.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let answer: serde_json::Value = serde_json::from_str(&call(Some(&case["request"].to_string()))).unwrap();
            assert_eq!(answer, case["response"], "{}", case["name"]);
        }
        let missing: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(missing["error"], "invalid");
    }
}
