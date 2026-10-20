//! Connection callbacks: stages, start and termination, and every control event, forwarded
//! to MoonBridge's `bridgeCl*` callbacks with the arguments `callbacks.c` passes.

use std::sync::atomic::{AtomicBool, Ordering};

use nightjar_core::control::{haptics, ConnectionStatus, ControlEvent};
use nightjar_core::session::{Listener, Stage};
use rubylight_protocol::control::HostTiming;

use crate::bridge::{self, Bridge};
use crate::extras::{cursor_shape_message, hdr_metadata_bytes};
use crate::{engine, platform};

pub struct Events {
    bridge: &'static Bridge,
    /// `connection_started` arrived; forwarded once the session is stored, so Java can use
    /// it from the callback as with the C engine.
    pub started: AtomicBool,
}

impl Events {
    pub fn new(bridge: &'static Bridge) -> Self {
        Self {
            bridge,
            started: AtomicBool::new(false),
        }
    }

    fn bytes_callback(
        &self,
        method: jni::objects::JStaticMethodID,
        lead: Option<jni::sys::jvalue>,
        bytes: &[u8],
    ) {
        let b = self.bridge;
        b.with_env(|env| {
            let array = env.byte_array_from_slice(bytes)?;
            let mut args = Vec::with_capacity(2);
            args.extend(lead);
            args.push(bridge::object(&array));
            b.void(env, method, &args);
            Ok(())
        });
    }
}

impl Listener for Events {
    fn stage_starting(&self, stage: Stage) {
        self.bridge
            .post(self.bridge.m.stage_starting, &[bridge::int(stage as i32)]);
    }

    fn stage_complete(&self, stage: Stage) {
        self.bridge
            .post(self.bridge.m.stage_complete, &[bridge::int(stage as i32)]);
    }

    fn stage_failed(&self, stage: Stage, error: i32) {
        self.bridge.post(
            self.bridge.m.stage_failed,
            &[bridge::int(stage as i32), bridge::int(error)],
        );
    }

    fn connection_started(&self) {
        self.started.store(true, Ordering::SeqCst);
    }

    fn connection_terminated(&self, error: i32) {
        platform::info(&format!("Connection terminated: {error}"));
        self.bridge
            .post(self.bridge.m.connection_terminated, &[bridge::int(error)]);
    }

    fn control_event(&self, event: ControlEvent) {
        let b = self.bridge;
        let m = &b.m;
        match event {
            ControlEvent::Rumble {
                controller,
                low_frequency,
                high_frequency,
            } => b.post(
                m.rumble,
                &[
                    bridge::short(controller),
                    bridge::short(low_frequency),
                    bridge::short(high_frequency),
                ],
            ),
            ControlEvent::RumbleTriggers {
                controller,
                left,
                right,
            } => b.post(
                m.rumble_triggers,
                &[
                    bridge::short(controller),
                    bridge::short(left),
                    bridge::short(right),
                ],
            ),
            ControlEvent::SetMotionEventState {
                controller,
                motion_type,
                report_rate_hz,
            } => b.post(
                m.motion_state,
                &[
                    bridge::short(controller),
                    bridge::byte(motion_type),
                    bridge::short(report_rate_hz),
                ],
            ),
            ControlEvent::SetControllerLed {
                controller,
                r,
                g,
                b: blue,
            } => b.post(
                m.controller_led,
                &[
                    bridge::short(controller),
                    bridge::byte(r),
                    bridge::byte(g),
                    bridge::byte(blue),
                ],
            ),
            ControlEvent::AdaptiveTriggers {
                controller,
                event_flags,
                type_left,
                type_right,
                left,
                right,
            } => {
                b.with_env(|env| {
                    let left = env.byte_array_from_slice(&left)?;
                    let right = env.byte_array_from_slice(&right)?;
                    b.void(
                        env,
                        m.adaptive_triggers,
                        &[
                            bridge::short(controller),
                            bridge::byte(event_flags),
                            bridge::byte(type_left),
                            bridge::byte(type_right),
                            bridge::object(&left),
                            bridge::object(&right),
                        ],
                    );
                    Ok(())
                });
            }
            ControlEvent::HdrMode { enabled, metadata } => {
                b.with_env(|env| {
                    // Metadata only with HDR on, as LiGetHdrMetadata answers.
                    let array = match metadata.filter(|_| enabled) {
                        Some(metadata) => {
                            env.byte_array_from_slice(&hdr_metadata_bytes(&metadata))?
                        }
                        None => jni::objects::JByteArray::default(),
                    };
                    b.void(
                        env,
                        m.hdr_mode,
                        &[
                            jni::objects::JValue::Bool(u8::from(enabled)).as_jni(),
                            bridge::object(&array),
                        ],
                    );
                    Ok(())
                });
            }
            ControlEvent::ControllerHaptics(pcm) => {
                b.with_env(|env| {
                    let samples = env.byte_array_from_slice(&pcm.samples)?;
                    b.void(
                        env,
                        m.controller_haptics,
                        &[
                            bridge::short(pcm.controller),
                            bridge::int(pcm.sequence as i32),
                            bridge::object(&samples),
                        ],
                    );
                    Ok(())
                });
            }
            ControlEvent::ControllerSpeaker(pcm) => {
                if let Some(payload) = haptics::encode(pcm.controller, pcm.sequence, &pcm.samples) {
                    self.bytes_callback(m.controller_audio, Some(bridge::int(0x5533)), &payload);
                }
            }
            ControlEvent::ControllerAudioControl(payload) => {
                self.bytes_callback(m.controller_audio, Some(bridge::int(0x5535)), &payload);
            }
            ControlEvent::HostTiming(records) => {
                self.bytes_callback(m.host_timing, None, &HostTiming::encode_report(&records));
            }
            ControlEvent::ClockSync { reply, received } => {
                engine::clock_sync(&reply, platform::monotonic_us_at(received));
            }
            ControlEvent::CursorShape(shape) => {
                self.bytes_callback(m.cursor_shape, None, &cursor_shape_message(&shape));
            }
            ControlEvent::CursorShapeRejected(reason) => {
                platform::warn(&format!("Dropping a cursor shape message: {reason}"));
            }
            ControlEvent::ConnectionStatus(status) => {
                let code = match status {
                    ConnectionStatus::Okay => 0,
                    ConnectionStatus::Poor => 1,
                };
                b.post(m.connection_status, &[bridge::int(code)]);
            }
            // The session reports terminations through connection_terminated.
            ControlEvent::Terminated { .. } => {}
        }
    }

    fn log(&self, message: &str) {
        platform::info(message);
    }
}
