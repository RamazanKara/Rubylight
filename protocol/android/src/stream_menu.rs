//! The stream menu's model for the app's `StreamMenu` (`com.limelight.ui.StreamMenuModel`): the
//! tile grid, the shortcuts' built-ins, the key picker and a chord's check come from
//! nightjar-policy, the same code desktop Nightjar draws its stream menu from. One call takes a
//! JSON request and answers JSON (`nightjar_policy::stream_menu::json`).

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

/// The answer to a request the app could not hand over, in the model's own error shape.
fn unreadable() -> String {
    serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string()
}

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_policy::stream_menu::json::call(request),
        None => unreadable(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_ui_StreamMenuModel_call(
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

    #[test]
    fn answers_through_the_shared_model_and_names_a_missing_request() {
        let answer: serde_json::Value = serde_json::from_str(&call(Some(
            r#"{"op":"layout","offers":[{"id":"overlay","on":false}],"shortcuts":0}"#,
        )))
        .unwrap();
        assert_eq!(answer["ok"]["grid"][0]["tile"]["hotkey"], "rb");
        assert_eq!(answer["ok"]["grid"][1], "add");
        let answer: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }

    /// The wire contract with the app: each request exactly as `StreamMenuModel.java` sends it, and
    /// the answer this entry gives. The app's `StreamMenuModelContractTest` sends and reads the same
    /// file, so a change on either side of the JNI call fails one of the two.
    #[test]
    fn answers_each_request_of_the_shared_contract_exactly() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("stream_menu_contract.json")).unwrap();
        assert!(cases.len() >= 12);
        for case in cases {
            let request = case["request"].to_string();
            let answer: serde_json::Value = serde_json::from_str(&call(Some(&request))).unwrap();
            assert_eq!(answer, case["response"], "{}", case["name"]);
        }
    }
}
