//! Play over the internet for the app's `com.limelight.computers.InternetPlay`: the same paths,
//! tunnel and words desktop Nightjar uses (`nightjar_core::internet`), one JSON call at a time.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_core::internet::json::call(request),
        None => serde_json::json!({"error": "the request is not a string"}).to_string(),
    }
}

/// Blocks up to `nightjar_core::internet::PUNCH_WAIT` for `connect`: call it off the main thread.
#[no_mangle]
pub extern "system" fn Java_com_limelight_computers_InternetPlay_call(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let request = native_api::string(&mut env, &request);
    match env.new_string(call(request.as_deref())) {
        Ok(answer) => answer.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reaches_the_shared_calls_and_a_missing_one_is_an_error() {
        let reply = r#"{"version":1,"enabled":true,"ipv6":["2001:db8::5"],
            "ports":{"http":47989,"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002}}"#;
        let request = serde_json::json!({"op": "addresses", "reply": reply}).to_string();
        let answer: serde_json::Value = serde_json::from_str(&call(Some(&request))).unwrap();
        assert_eq!(answer["ok"]["ipv6"][0], "2001:db8::5");
        assert!(answer["ok"]["mapped"].is_null());
        let missing: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert!(missing["error"].is_string());
    }
}
