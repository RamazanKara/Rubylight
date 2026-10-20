//! The encrypted control stream: pings, loss and FEC reports, IDR and RFI requests, rumble,
//! triggers, LEDs, motion, HDR, controller haptics and audio, Rubylight's 0x55xx messages and
//! termination.
//!
//! Ported from moonlight-common-c's `ControlStream.c` (GPL-3.0, Cameron Gutman and
//! contributors) with Nightjar's patches (controller haptics, host termination, disconnect
//! intercept, control payload and teardown, host timing, network grace, cursor shapes, clock
//! sync) and `android_control.c`. Only the encrypted stream (GFE 3.22 and every Sunshine
//! host) is supported; GFE's older TCP and plaintext generations are not.
//!
//! [`ControlSession`] is the protocol without sockets. [`driver`] runs it, and the input
//! queue, over ENet on a thread of the caller's choosing.

pub mod driver;
pub mod events;
pub mod haptics;
pub mod messages;
pub mod negotiation;
pub mod session;
pub mod wire;

pub use events::{ConnectionStatus, ControlEvent, CursorShape, HdrMetadata};
pub use messages::{ClientMessage, FrameFecStatus};
pub use negotiation::HostCapabilities;
pub use session::{ControlSession, Outgoing, SendError, SessionOptions};
pub use wire::{channel, ptype, Delivery};
