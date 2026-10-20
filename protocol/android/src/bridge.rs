//! Calls into `com.limelight.nvstream.jni.MoonBridge`'s static `bridge*` callbacks, the same
//! ones `callbacks.c` calls, so the renderers and the connection listener serve both engines
//! unchanged. Session threads attach to the VM on first use and detach when they end.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JStaticMethodID, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::jvalue;
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
    pub ar_init: JStaticMethodID,
    pub ar_start: JStaticMethodID,
    pub ar_stop: JStaticMethodID,
    pub ar_cleanup: JStaticMethodID,
    pub ar_play: JStaticMethodID,
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
    pub cursor_shape: JStaticMethodID,
}

pub struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
    pub m: Methods,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// Looks up MoonBridge and its callbacks; called from `RustBridge.init()` on a Java thread,
/// where the app's class loader can find MoonBridge.
pub fn init(env: &mut JNIEnv) -> jni::errors::Result<()> {
    if BRIDGE.get().is_some() {
        return Ok(());
    }
    let class = env.find_class("com/limelight/nvstream/jni/MoonBridge")?;
    let mut id = |name: &str, signature: &str| env.get_static_method_id(&class, name, signature);
    let m = Methods {
        dr_setup: id("bridgeDrSetup", "(IIII)I")?,
        dr_setup_packing: id("bridgeDrSetupPacking", "(IIIII)I")?,
        display_message: id("bridgeClDisplayMessage", "(Ljava/lang/String;)V")?,
        dr_start: id("bridgeDrStart", "()V")?,
        dr_stop: id("bridgeDrStop", "()V")?,
        dr_cleanup: id("bridgeDrCleanup", "()V")?,
        dr_submit: id("bridgeDrSubmitDecodeUnit", "([BIIIICJJJJ)I")?,
        ar_init: id("bridgeArInit", "(III)I")?,
        ar_start: id("bridgeArStart", "()V")?,
        ar_stop: id("bridgeArStop", "()V")?,
        ar_cleanup: id("bridgeArCleanup", "()V")?,
        ar_play: id("bridgeArPlaySample", "([SI)V")?,
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
        cursor_shape: id("bridgeClCursorShape", "([B)V")?,
    };
    let bridge = Bridge {
        vm: env.get_java_vm()?,
        class: env.new_global_ref(&class)?,
        m,
    };
    let _ = BRIDGE.set(bridge);
    Ok(())
}

pub fn get() -> Option<&'static Bridge> {
    BRIDGE.get()
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
