//! The profile kept for a screen, for the app's `com.limelight.preferences.StreamLayers`: the same
//! rule desktop Nightjar uses (`nightjar_policy::display_profile`). One JSON request in, one JSON
//! answer out (`display_profile::json::call`).

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_policy::display_profile::json::call(request),
        None => serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayProfile(
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

    /// The request exactly as `StreamLayers.screenRequest` writes it, and the answer it reads.
    #[test]
    fn answers_the_request_stream_layers_sends() {
        let request = r#"{"rules":[{"profile":"2","display":{"id":"builtin","name":""}},{"profile":"5","display":{"id":"external:HDMI Screen","name":"HDMI Screen"}}],"screen":{"id":"external:HDMI Screen","name":"HDMI Screen"}}"#;
        assert_eq!(call(Some(request)), r#"{"profile":"5"}"#);
        let builtin = r#"{"rules":[{"profile":"2","display":{"id":"builtin","name":""}}],"screen":{"id":"builtin","name":""}}"#;
        assert_eq!(call(Some(builtin)), r#"{"profile":"2"}"#);
        let answer: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }
}
