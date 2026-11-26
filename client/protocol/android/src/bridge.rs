//! Calls into `com.limelight.nvstream.jni.MoonBridge`'s static `bridge*` callbacks, the same
//! ones `callbacks.c` calls, so the renderers and the connection listener serve both engines
//! unchanged. Session threads attach to the VM on first use and detach when they end.

use std::sync::{Arc, Mutex};

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JStaticMethodID, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jboolean, jvalue};
use jni::{JNIEnv, JavaVM};

use crate::platform;

pub struct Methods {
    pub dr_setup: JStaticMethodID,
    pub dr_setup_packing: JStaticMethodID,
    pub display_message: JStaticMethodID,
    pub dr_start: JStaticMethodID,
    pub dr_stop: JStaticMethodID,
    pub dr_cleanup: JStaticMethodID,
    pub dr_submit: JStaticMethodID,
    pub dr_submit_direct: JStaticMethodID,
    pub dr_submit_raw: JStaticMethodID,
    pub dr_chunks: JStaticMethodID,
    pub dr_chunks_direct: JStaticMethodID,
    pub dr_chunks_raw: JStaticMethodID,
    pub ar_init: JStaticMethodID,
    pub ar_start: JStaticMethodID,
    pub ar_stop: JStaticMethodID,
    pub ar_cleanup: JStaticMethodID,
    #[allow(dead_code)]
    pub ar_play: JStaticMethodID,
    pub ar_play_direct: JStaticMethodID,
    pub ar_play_pending: JStaticMethodID,
    pub ar_init_encoded: JStaticMethodID,
    pub ar_play_encoded: JStaticMethodID,
    pub stage_starting: JStaticMethodID,
    pub stage_complete: JStaticMethodID,
    pub stage_failed: JStaticMethodID,
    pub connection_started: JStaticMethodID,
    pub connection_terminated: JStaticMethodID,
    pub rumble: JStaticMethodID,
    pub connection_status: JStaticMethodID,
    pub hdr_mode: JStaticMethodID,
    pub rumble_triggers: JStaticMethodID,
    pub motion_state: JStaticMethodID,
    pub controller_led: JStaticMethodID,
    pub adaptive_triggers: JStaticMethodID,
    pub controller_haptics: JStaticMethodID,
    pub controller_audio: JStaticMethodID,
    pub host_timing: JStaticMethodID,
    pub link_notice: JStaticMethodID,
    pub rate_change: JStaticMethodID,
    pub cursor_shape: JStaticMethodID,
    pub steam_haptic: JStaticMethodID,
    pub steam_settings: JStaticMethodID,
    pub text_input: JStaticMethodID,
    pub usb_changed: JStaticMethodID,
    pub clipboard_sync: JStaticMethodID,
    pub clipboard_sync_refused: JStaticMethodID,
    pub dvr_state: JStaticMethodID,
    pub dvr_clip: JStaticMethodID,
}

pub struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
    pub m: Methods,
}

static BRIDGE: Mutex<Option<Arc<Bridge>>> = Mutex::new(None);

/// Looks up MoonBridge's callbacks on the caller's class; called from `NightjarJni.rustBridgeInit()`
/// when a stream starts so Robolectric sandboxes bind their own `MoonBridge` classloader without
/// triggering `MoonBridge.<clinit>` during `System.loadLibrary`.
pub fn init(env: &mut JNIEnv, class: &JClass) -> jni::errors::Result<()> {
    if class.is_null() {
        return Err(jni::errors::Error::NullPtr("MoonBridge class"));
    }
    {
        let guard = BRIDGE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = guard.as_ref()
            && env
                .is_same_object(existing.class.as_obj(), class)
                .unwrap_or(false)
        {
            return Ok(());
        }
    }
    let mut id = |name: &str, signature: &str| env.get_static_method_id(class, name, signature);
    let m = Methods {
        dr_setup: id("bridgeDrSetup", "(IIII)I")?,
        dr_setup_packing: id("bridgeDrSetupPacking", "(IIIII)I")?,
        display_message: id("bridgeClDisplayMessage", "(Ljava/lang/String;)V")?,
        dr_start: id("bridgeDrStart", "()V")?,
        dr_stop: id("bridgeDrStop", "()V")?,
        dr_cleanup: id("bridgeDrCleanup", "()V")?,
        dr_submit: id("bridgeDrSubmitDecodeUnit", "([BIIIICJJJJ)I")?,
        dr_submit_direct: id(
            "bridgeDrSubmitDirectDecodeUnit",
            "(Ljava/nio/ByteBuffer;IIIICJJJJ)I",
        )?,
        dr_submit_raw: id("drSubmitRawDecodeUnit", "(JIIIICJJJJ)I")?,
        dr_chunks: id("bridgeDrSubmitChunks", "([BIZ)V")?,
        dr_chunks_direct: id("bridgeDrSubmitChunksDirect", "(Ljava/nio/ByteBuffer;IZ)V")?,
        dr_chunks_raw: id("drSubmitChunksRaw", "(JIZ)V")?,
        ar_init: id("bridgeArInit", "(III)I")?,
        ar_start: id("bridgeArStart", "()V")?,
        ar_stop: id("bridgeArStop", "()V")?,
        ar_cleanup: id("bridgeArCleanup", "()V")?,
        ar_play: id("bridgeArPlaySample", "([SI)V")?,
        ar_play_direct: id("arPlaySampleDirect", "(JII)I")?,
        ar_play_pending: id("arPlaySamplePending", "([SII)V")?,
        ar_init_encoded: id("bridgeArInitEncoded", "(II)I")?,
        ar_play_encoded: id("bridgeArPlayEncoded", "([BI)V")?,
        stage_starting: id("bridgeClStageStarting", "(I)V")?,
        stage_complete: id("bridgeClStageComplete", "(I)V")?,
        stage_failed: id("bridgeClStageFailed", "(II)V")?,
        connection_started: id("bridgeClConnectionStarted", "()V")?,
        connection_terminated: id("bridgeClConnectionTerminated", "(I)V")?,
        rumble: id("bridgeClRumble", "(SSS)V")?,
        connection_status: id("bridgeClConnectionStatusUpdate", "(I)V")?,
        hdr_mode: id("bridgeClSetHdrMode", "(Z[B)V")?,
        rumble_triggers: id("bridgeClRumbleTriggers", "(SSS)V")?,
        motion_state: id("bridgeClSetMotionEventState", "(SBS)V")?,
        controller_led: id("bridgeClSetControllerLED", "(SBBB)V")?,
        adaptive_triggers: id("bridgeClSetAdaptiveTriggers", "(SBBB[B[B)V")?,
        controller_haptics: id("bridgeClControllerHaptics", "(SI[B)V")?,
        controller_audio: id("bridgeClControllerAudio", "(I[B)V")?,
        host_timing: id("bridgeClHostTiming", "([B)V")?,
        link_notice: id("bridgeClLinkNotice", "(III)V")?,
        rate_change: id("bridgeClRateChange", "(IIIIIII)V")?,
        cursor_shape: id("bridgeClCursorShape", "([B)V")?,
        steam_haptic: id("bridgeClSteamHaptic", "(S[B)V")?,
        steam_settings: id("bridgeClSteamSettings", "(SB[B)V")?,
        text_input: id("bridgeClTextInput", "(IIII)V")?,
        usb_changed: id("bridgeClUsbChanged", "()V")?,
        clipboard_sync: id("bridgeClClipboardSync", "(Ljava/lang/String;)V")?,
        clipboard_sync_refused: id("bridgeClClipboardSyncRefused", "(I)V")?,
        dvr_state: id("bridgeClDvrState", "(IIIIIIJJ)V")?,
        dvr_clip: id("bridgeClDvrClip", "(IIIIJ)V")?,
    };
    let bridge = Arc::new(Bridge {
        vm: env.get_java_vm()?,
        class: env.new_global_ref(class)?,
        m,
    });
    *BRIDGE.lock().unwrap_or_else(|p| p.into_inner()) = Some(bridge);
    Ok(())
}

pub fn get() -> Option<Arc<Bridge>> {
    BRIDGE.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn int(value: i32) -> jvalue {
    JValue::Int(value).as_jni()
}

pub fn short(value: u16) -> jvalue {
    // Java has no unsigned short; the callbacks treat the bits as unsigned, as in callbacks.c.
    JValue::Short(value as i16).as_jni()
}

pub fn byte(value: u8) -> jvalue {
    JValue::Byte(value as i8).as_jni()
}

pub fn long(value: i64) -> jvalue {
    JValue::Long(value).as_jni()
}

pub fn object(value: &JObject) -> jvalue {
    JValue::Object(value).as_jni()
}

/// A global reference to a `byte[]` as the array type.
pub fn byte_array(array: &GlobalRef) -> &JByteArray<'static> {
    <&JByteArray>::from(array.as_obj())
}

/// `&[u8]` as JNI's signed bytes.
pub fn jbytes(bytes: &[u8]) -> &[i8] {
    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len()) }
}

impl Bridge {
    /// This thread's JNIEnv, attaching it until it exits if needed.
    pub fn env(&self) -> Option<JNIEnv<'_>> {
        match self.vm.attach_current_thread_permanently() {
            Ok(env) => Some(env),
            Err(e) => {
                platform::error(&format!("Could not attach a thread to the VM: {e}"));
                None
            }
        }
    }

    fn class(&self) -> &JClass<'static> {
        <&JClass>::from(self.class.as_obj())
    }

    fn call(
        &self,
        env: &mut JNIEnv,
        method: JStaticMethodID,
        ret: ReturnType,
        args: &[jvalue],
    ) -> Option<i32> {
        let result = unsafe { env.call_static_method_unchecked(self.class(), method, ret, args) };
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return None;
        }
        match result {
            Ok(value) => Some(value.i().unwrap_or(0)),
            Err(e) => {
                platform::error(&format!("MoonBridge callback failed: {e}"));
                None
            }
        }
    }

    /// Calls a `void` callback; Java exceptions are logged and cleared.
    pub fn void(&self, env: &mut JNIEnv, method: JStaticMethodID, args: &[jvalue]) {
        let _ = self.call(env, method, ReturnType::Primitive(Primitive::Void), args);
    }

    /// Calls an `int` callback; `None` when it threw.
    pub fn int(&self, env: &mut JNIEnv, method: JStaticMethodID, args: &[jvalue]) -> Option<i32> {
        self.call(env, method, ReturnType::Primitive(Primitive::Int), args)
    }

    /// Calls a `void` callback from any thread, with local references freed afterwards.
    pub fn post(&self, method: JStaticMethodID, args: &[jvalue]) {
        if let Some(mut env) = self.env() {
            self.void(&mut env, method, args);
        }
    }

    /// Runs `f` in a local reference frame on this thread's JNIEnv, so long-lived session
    /// threads do not accumulate local references.
    pub fn with_env<R: Default>(&self, f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<R>) -> R {
        let Some(mut env) = self.env() else {
            return R::default();
        };
        match env.with_local_frame(8, |env| f(env)) {
            Ok(value) => value,
            Err(e) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                platform::error(&format!("JNI call failed: {e}"));
                R::default()
            }
        }
    }
}

fn exact_key16(bytes: Vec<u8>) -> Option<[u8; 16]> {
    bytes.try_into().ok()
}

pub(crate) fn key16(env: &mut JNIEnv, array: &JByteArray) -> Option<[u8; 16]> {
    if array.is_null() {
        return None;
    }
    let bytes = env.convert_byte_array(array).ok()?;
    exact_key16(bytes)
}

pub(crate) fn jbool(value: bool) -> jboolean {
    jboolean::from(value)
}

pub(crate) fn string(env: &mut JNIEnv, value: &jni::objects::JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[cfg(test)]
mod tests {
    use super::exact_key16;

    #[test]
    fn key16_accepts_only_exact_16_bytes() {
        assert_eq!(exact_key16(vec![7u8; 16]), Some([7u8; 16]));
        assert_eq!(exact_key16(vec![7u8; 15]), None);
        assert_eq!(exact_key16(vec![7u8; 17]), None);
        assert_eq!(exact_key16(Vec::new()), None);
    }
}
