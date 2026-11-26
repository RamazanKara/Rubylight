//! The control worker: ENet control peers, input dispatch, feedback and roaming for every
//! session.

mod audio;
mod clipboard;
mod event_loop;
mod extensions;
mod haptics;
mod input;
mod peer;
mod ps_speaker;
mod qos;
mod socket;

pub use haptics::{RUMBLE_DATA, RUMBLE_TRIGGER_DATA, feedback_packets};
pub use input::{PadsWait, attach_pads, controllers_back, controllers_wait};

#[cfg(test)]
pub(super) use extensions::server_command;
#[cfg(test)]
pub(super) use haptics::{
    FEEDBACK_CHANNEL, MOTION_EVENT_REQUEST, PadSends, RUMBLE_MARKER, RumbleForwarded,
    haptics_negotiated, motion_requests, pad_reports,
};
#[cfg(test)]
pub(super) use input::{
    INPUT_OVERFLOW, INPUT_QUEUE, drop_uninjectable_input, first_steam_attributes_refusal,
    hold_pad_input, queue_input,
};
#[cfg(any(test, feature = "test-support"))]
pub(super) use peer::ControlPeer;
#[cfg(test)]
pub(super) use peer::{PeerConfig, ScaleLookup};
#[cfg(any(test, feature = "test-support"))]
pub(super) use socket::ControlSocket;
#[cfg(test)]
pub(super) use socket::{
    CONTROL_EVENT_LIMIT, SWALLOWED_PER_RECEIVE, UNATTENDED_SWEEP, UnattendedSweep,
    control_wait_timeout, end_unattended_sessions, expects_enet_control, expire_controllers,
};
