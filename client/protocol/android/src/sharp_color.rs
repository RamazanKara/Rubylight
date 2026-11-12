//! "Sharper colored text" for the app's `SharpColor` (`com.limelight.binding.video.SharpColor`): native
//! 4:4:4 first, then packed full chroma, decided by nightjar-core's `sharp_color::decide`, the code
//! desktop Nightjar runs too. One call takes a JSON request and answers JSON; the wire is
//! `nightjar_core::sharp_color::json`, held to `client/src/sharp_color_contract.json`, which the app's
//! `SharpColorTest` reads as well.
//!
//! The device's packed modes cost a verification decode, so the first request sends `device_modes`
//! null: when the decision needs them, the answer is `{"ok": {"need_device_modes": true}}` and the app
//! asks again with them. A decision that never needs them answers at once.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_core::sharp_color::json::call(request),
        None => serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_video_SharpColor_call(
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
    use serde_json::Value;

    fn answer(request: &str) -> Value {
        serde_json::from_str(&call(Some(request))).unwrap()
    }

    /// The JNI entry answers each request of the shared contract exactly as the file says.
    #[test]
    fn the_jni_entry_speaks_the_shared_contract() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../../client/src/sharp_color_contract.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            assert_eq!(answer(&case["request"].to_string()), case["response"], "{}", case["name"]);
        }
    }

    /// The device's modes are asked for only when the packed step needs them, and the second request
    /// with them decides.
    #[test]
    fn the_device_modes_are_asked_for_only_when_needed() {
        let base = r#""op":"decide","auto":true,"hdr":false,"sdr10":false,"pyrowave":false,"packed_engine":true,
            "base":257,"decodable":0,"decoder_reason":"no 4:4:4 profile","host_scm":524545,"host_packed":1,
            "packed_refused_before":false"#;
        assert_eq!(answer(&format!("{{{base},\"device_modes\":null}}"))["ok"]["need_device_modes"], true);
        assert_eq!(answer(&format!("{{{base},\"device_modes\":1}}"))["ok"]["pick"], "packed");
        // Off never asks.
        let off = base.replace(r#""auto":true"#, r#""auto":false"#);
        assert_eq!(answer(&format!("{{{off},\"device_modes\":null}}"))["ok"]["why"], "off");
    }

    #[test]
    fn a_malformed_request_is_named() {
        for request in [
            "not json",
            r#"{"op":"something else"}"#,
            r#"{"op":"decide","auto":"yes"}"#,
            r#"{"op":"decide","auto":true,"hdr":false,"sdr10":false,"pyrowave":false,"packed_engine":true,"base":257,
                "decodable":0,"host_scm":0,"host_packed":300,"packed_refused_before":false,"device_modes":null}"#,
        ] {
            assert_eq!(answer(request)["error"], "invalid", "{request}");
        }
        let missing: Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(missing["error"], "invalid");
    }
}
