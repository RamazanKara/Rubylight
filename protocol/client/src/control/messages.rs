//! Client-to-host control messages: payloads, channels and delivery, as
//! moonlight-common-c's `ControlStream.c` and Nightjar's `android_control.c` send them.

use rubylight_protocol::control::{
    ClockSyncRequest, DisplayCaps, HostTimingRequest, LinkNoticeRequest, Reconfigure,
};
use rubylight_protocol::fec_report::FecReport;
use rubylight_protocol::phase_lock;
use rubylight_protocol::pipeline_rates::PipelineRatesRequest;
use rubylight_protocol::rate_change::RateChangeRequest;
use rubylight_protocol::usb::UsbMessage;

use super::wire::{channel, ptype, Delivery};
use crate::control::haptics;

/// Largest edge of a cursor image the client accepts (docs/protocol/cursor-shape.md).
pub const CURSOR_SHAPE_MAX_EDGE: u16 = 96;
const CURSOR_SHAPE_VERSION: u8 = 1;

/// Sunshine's per-frame FEC report (`SS_FRAME_FEC_STATUS`). All fields are big-endian on
/// the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameFecStatus {
    pub frame_index: u32,
    pub highest_received_sequence: u16,
    pub next_contiguous_sequence: u16,
    pub missing_before_highest_received: u16,
    pub total_data_packets: u16,
    pub total_parity_packets: u16,
    pub received_data_packets: u16,
    pub received_parity_packets: u16,
    pub fec_percentage: u8,
    pub multi_fec_block_index: u8,
    pub multi_fec_block_count: u8,
}

impl FrameFecStatus {
    pub const BYTES: usize = 21;

    pub fn encode(&self) -> [u8; Self::BYTES] {
        let mut out = [0; Self::BYTES];
        out[..4].copy_from_slice(&self.frame_index.to_be_bytes());
        let words = [
            self.highest_received_sequence,
            self.next_contiguous_sequence,
            self.missing_before_highest_received,
            self.total_data_packets,
            self.total_parity_packets,
            self.received_data_packets,
            self.received_parity_packets,
        ];
        for (i, word) in words.iter().enumerate() {
            out[4 + i * 2..6 + i * 2].copy_from_slice(&word.to_be_bytes());
        }
        out[18] = self.fec_percentage;
        out[19] = self.multi_fec_block_index;
        out[20] = self.multi_fec_block_count;
        out
    }
}

/// A message the client sends on the control stream. Input packets go through
/// [`crate::input`] instead.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientMessage {
    /// Start A; on the encrypted stream this is an IDR request with a two-byte payload.
    StartA,
    StartB,
    /// Every 100 ms, reliable so ENet keeps measuring the round trip.
    PeriodicPing,
    RequestIdrFrame,
    /// Reference frame invalidation (`SS_RFI_REQUEST`) for lost frames `first..=last`.
    InvalidateReferenceFrames {
        first: u32,
        last: u32,
    },
    /// Acknowledges a long-term reference frame (`SS_LTR_FRAME_ACK`).
    LtrFrameAck {
        frame: u32,
    },
    FrameFecStatus(FrameFecStatus),
    /// Apollo's server command: one or four bytes.
    ServerCommand(Vec<u8>),
    PhaseLockReport(phase_lock::Report),
    DisplayCaps(DisplayCaps),
    Reconfigure(Reconfigure),
    HostTimingRequest(HostTimingRequest),
    ClockSync(ClockSyncRequest),
    /// Asks the host to say when it lowers the video bitrate for the network (0x5538).
    LinkNoticeRequest(LinkNoticeRequest),
    /// The once-a-second video FEC loss report (0x5539).
    FecReport(FecReport),
    /// Asks the host for its pipeline frame rates (0x553D) every interval, or to stop.
    PipelineRatesRequest(PipelineRatesRequest),
    /// Asks the host for a new video bitrate, frame rate or both (0x553C), on a stream
    /// launched with `rlRateChange=1`. The host answers each with a
    /// [`ControlEvent::RateChange`](super::events::ControlEvent).
    RateChange(RateChangeRequest),
    /// Asks the host to send its cursor image (0x5541) and leave it out of the video, or to
    /// stop.
    CursorShapeRequest {
        enabled: bool,
    },
    /// Asks the host to say when one of its text fields takes or loses focus (0x553B), or
    /// to stop.
    TextInputRequest(rubylight_protocol::text_input::TextInputRequest),
    /// The client's keyboard layout (0x553E), at stream start and when it changes.
    KeyboardLayout(rubylight_protocol::keyboard_layout::KeyboardLayout),
    /// DualSense microphone PCM (0x5534), in the controller haptics layout.
    ControllerMicrophone(Vec<u8>),
    /// USB controller audio status (0x5535): which endpoints the client has open.
    ControllerAudioStatus {
        controller: u16,
        flags: u8,
    },
    /// USB forwarding (0x553A): an attach request or a detach. [`crate::usb::Forwarder`]
    /// makes them.
    Usb(UsbMessage),
    /// A clipboard item or the hello that starts the host's copies (0x5508), as
    /// [`super::clipboard_sync::ClipboardSync`] makes them.
    ClipboardSync(rubylight_protocol::clipboard_sync::Frame),
}

impl ClientMessage {
    pub fn message_type(&self) -> u16 {
        match self {
            Self::StartA | Self::RequestIdrFrame => ptype::REQUEST_IDR_FRAME,
            Self::StartB => ptype::START_B,
            Self::PeriodicPing => ptype::PERIODIC_PING,
            Self::InvalidateReferenceFrames { .. } => ptype::INVALIDATE_REF_FRAMES,
            Self::LtrFrameAck { .. } => ptype::LTR_FRAME_ACK,
            Self::FrameFecStatus(_) => ptype::FRAME_FEC_STATUS,
            Self::ServerCommand(_) => ptype::SERVER_COMMAND,
            Self::PhaseLockReport(_) => ptype::PHASE_LOCK_REPORT,
            Self::DisplayCaps(_) => ptype::DISPLAY_CAPS,
            Self::Reconfigure(_) => ptype::RECONFIGURE,
            Self::HostTimingRequest(_) => ptype::HOST_TIMING,
            Self::ClockSync(_) => ptype::CLOCK_SYNC,
            Self::LinkNoticeRequest(_) => ptype::LINK_NOTICE,
            Self::FecReport(_) => ptype::FEC_REPORT,
            Self::PipelineRatesRequest(_) => ptype::PIPELINE_RATES,
            Self::RateChange(_) => ptype::RATE_CHANGE,
            Self::CursorShapeRequest { .. } => ptype::CURSOR_SHAPE_REQUEST,
            Self::TextInputRequest(_) => ptype::TEXT_INPUT,
            Self::KeyboardLayout(_) => ptype::KEYBOARD_LAYOUT,
            Self::ControllerMicrophone(_) => ptype::CONTROLLER_MICROPHONE,
            Self::ControllerAudioStatus { .. } => ptype::CONTROLLER_AUDIO_CONTROL,
            Self::Usb(_) => ptype::USB,
            Self::ClipboardSync(_) => ptype::CLIPBOARD_SYNC,
        }
    }

    pub fn payload(&self) -> Vec<u8> {
        match self {
            Self::StartA | Self::RequestIdrFrame => vec![0, 0],
            Self::StartB => vec![0],
            // Length 4, then a zero timestamp.
            Self::PeriodicPing => vec![4, 0, 0, 0, 0, 0, 0, 0],
            Self::InvalidateReferenceFrames { first, last } => {
                let mut out = vec![0; 24];
                out[..4].copy_from_slice(&first.to_le_bytes());
                out[8..12].copy_from_slice(&last.to_le_bytes());
                out
            }
            Self::LtrFrameAck { frame } => {
                let mut out = vec![0; 8];
                out[..4].copy_from_slice(&frame.to_le_bytes());
                out
            }
            Self::FrameFecStatus(status) => status.encode().to_vec(),
            Self::ServerCommand(bytes) | Self::ControllerMicrophone(bytes) => bytes.clone(),
            Self::PhaseLockReport(report) => report.encode().to_vec(),
            Self::DisplayCaps(caps) => caps.encode().to_vec(),
            Self::Reconfigure(request) => request.encode().to_vec(),
            Self::HostTimingRequest(request) => request.encode().to_vec(),
            Self::ClockSync(request) => request.encode().to_vec(),
            Self::LinkNoticeRequest(request) => request.encode().to_vec(),
            Self::FecReport(report) => report.encode().to_vec(),
            Self::PipelineRatesRequest(request) => request.encode().to_vec(),
            Self::RateChange(request) => request.encode().to_vec(),
            Self::CursorShapeRequest { enabled } => {
                let edge = CURSOR_SHAPE_MAX_EDGE.to_le_bytes();
                vec![CURSOR_SHAPE_VERSION, u8::from(*enabled), edge[0], edge[1]]
            }
            Self::TextInputRequest(request) => request.encode().to_vec(),
            // is_valid refuses a layout that does not encode.
            Self::KeyboardLayout(layout) => layout.encode().unwrap_or_default(),
            Self::ControllerAudioStatus { controller, flags } => {
                let id = controller.to_le_bytes();
                vec![1, 1, id[0], id[1], *flags, 0, 0, 0]
            }
            // `is_valid` refused what does not encode.
            Self::Usb(message) => message.encode().unwrap_or_default(),
            Self::ClipboardSync(frame) => frame.encode().unwrap_or_default(),
        }
    }

    pub fn channel(&self) -> u8 {
        match self {
            Self::RequestIdrFrame
            | Self::InvalidateReferenceFrames { .. }
            | Self::LtrFrameAck { .. } => channel::URGENT,
            Self::ServerCommand(_)
            | Self::ControllerMicrophone(_)
            | Self::ControllerAudioStatus { .. }
            | Self::Usb(_) => channel::EXTENSION,
            _ => channel::GENERIC,
        }
    }

    pub fn delivery(&self) -> Delivery {
        match self {
            Self::FrameFecStatus(_) | Self::ClockSync(_) | Self::ControllerMicrophone(_) => {
                Delivery::Unsequenced
            }
            // A lost report is replaced half a second later, a lost FEC report a second later.
            Self::PhaseLockReport(_) | Self::FecReport(_) => Delivery::Unreliable,
            _ => Delivery::Reliable,
        }
    }

    /// The `a=x-rl-control` id the host must have announced before this may be sent.
    pub fn announcement(&self) -> Option<u16> {
        match self {
            Self::PhaseLockReport(_)
            | Self::DisplayCaps(_)
            | Self::Reconfigure(_)
            | Self::HostTimingRequest(_)
            | Self::ClockSync(_)
            | Self::LinkNoticeRequest(_)
            | Self::FecReport(_)
            | Self::PipelineRatesRequest(_)
            | Self::RateChange(_)
            | Self::CursorShapeRequest { .. }
            | Self::TextInputRequest(_)
            | Self::KeyboardLayout(_)
            | Self::ControllerMicrophone(_)
            | Self::ControllerAudioStatus { .. }
            | Self::Usb(_)
            | Self::ClipboardSync(_) => Some(self.message_type()),
            _ => None,
        }
    }

    /// Whether the message needs a Sunshine host on the encrypted stream.
    pub fn sunshine_only(&self) -> bool {
        !matches!(
            self,
            Self::StartA
                | Self::StartB
                | Self::PeriodicPing
                | Self::RequestIdrFrame
                | Self::InvalidateReferenceFrames { .. }
        )
    }

    /// Checks the payload rules `android_control.c` enforces before sending.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::ServerCommand(bytes) => matches!(bytes.len(), 1 | 4),
            Self::InvalidateReferenceFrames { first, last } => first <= last,
            Self::Reconfigure(request) => {
                request.width > 0 && request.height > 0 && request.fps_millihz > 0
            }
            Self::ControllerMicrophone(bytes) => haptics::validate(bytes),
            Self::ControllerAudioStatus { controller, flags } => {
                *controller < 16 && flags & !15 == 0
            }
            Self::KeyboardLayout(layout) => layout.encode().is_some(),
            Self::RateChange(request) => request.valid(),
            Self::ClipboardSync(frame) => frame.encode().is_some(),
            // Only the client's kinds, and only what encodes.
            Self::Usb(message) => {
                matches!(
                    message,
                    UsbMessage::AttachRequest { .. } | UsbMessage::Detach { .. }
                ) && message.encode().is_some()
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::{controller_audio, fec_status};

    #[test]
    fn fixed_payloads_match_the_c() {
        assert_eq!(ClientMessage::StartA.payload(), [0, 0]);
        assert_eq!(ClientMessage::StartA.message_type(), 0x0302);
        assert_eq!(ClientMessage::StartB.payload(), [0]);
        assert_eq!(ClientMessage::StartB.message_type(), 0x0307);
        assert_eq!(
            ClientMessage::PeriodicPing.payload(),
            [4, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(ClientMessage::PeriodicPing.delivery(), Delivery::Reliable);
        let rfi = ClientMessage::InvalidateReferenceFrames {
            first: 0x0102,
            last: 0x0304,
        };
        assert_eq!(
            rfi.payload(),
            [2, 1, 0, 0, 0, 0, 0, 0, 4, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(rfi.channel(), channel::URGENT);
        assert_eq!(
            ClientMessage::LtrFrameAck { frame: 9 }.payload(),
            [9, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            ClientMessage::CursorShapeRequest { enabled: true }.payload(),
            [1, 1, 96, 0]
        );
        assert_eq!(
            ClientMessage::CursorShapeRequest { enabled: false }.payload(),
            [1, 0, 96, 0]
        );
        assert_eq!(
            ClientMessage::ClockSync(ClockSyncRequest::default()).delivery(),
            Delivery::Unsequenced
        );
        assert_eq!(
            ClientMessage::PhaseLockReport(phase_lock::Report::default()).delivery(),
            Delivery::Unreliable
        );
        assert!(!ClientMessage::ServerCommand(vec![1, 2]).is_valid());
        assert!(ClientMessage::ServerCommand(vec![1, 2, 3, 4]).is_valid());
        assert_eq!(
            ClientMessage::ServerCommand(vec![1]).channel(),
            channel::EXTENSION
        );
    }

    #[test]
    fn fec_status_parses_on_the_host() {
        let status = FrameFecStatus {
            frame_index: 0x0102_0304,
            highest_received_sequence: 40,
            next_contiguous_sequence: 38,
            missing_before_highest_received: 2,
            total_data_packets: 30,
            total_parity_packets: 10,
            received_data_packets: 28,
            received_parity_packets: 10,
            fec_percentage: 33,
            multi_fec_block_index: 0,
            multi_fec_block_count: 1,
        };
        let parsed =
            fec_status::Status::parse(&ClientMessage::FrameFecStatus(status).payload()).unwrap();
        assert_eq!(parsed.frame, 0x0102_0304);
        assert_eq!(
            (parsed.data, parsed.parity, parsed.received_data),
            (30, 10, 28)
        );
        assert_eq!((parsed.percentage, parsed.block, parsed.blocks), (33, 0, 1));
    }

    #[test]
    fn controller_audio_parses_on_the_host() {
        let status = ClientMessage::ControllerAudioStatus {
            controller: 3,
            flags: 5,
        };
        assert!(status.is_valid());
        assert_eq!(controller_audio::status(&status.payload()), Some((3, 5)));
        assert_eq!(status.delivery(), Delivery::Reliable);
        assert!(!ClientMessage::ControllerAudioStatus {
            controller: 16,
            flags: 0
        }
        .is_valid());
        assert!(!ClientMessage::ControllerAudioStatus {
            controller: 1,
            flags: 16
        }
        .is_valid());

        let pcm = haptics::encode(2, 77, &[1, 0, 2, 0, 3, 0, 4, 0]).unwrap();
        let mic = ClientMessage::ControllerMicrophone(pcm.clone());
        assert!(mic.is_valid());
        assert_eq!(mic.delivery(), Delivery::Unsequenced);
        let (id, sequence, samples) = controller_audio::pcm(&pcm).unwrap();
        assert_eq!(
            (id, sequence, samples),
            (2, 77, &[1, 0, 2, 0, 3, 0, 4, 0][..])
        );
        assert!(!ClientMessage::ControllerMicrophone(pcm[..pcm.len() - 1].to_vec()).is_valid());
    }

    #[test]
    fn usb_messages_go_reliably_and_only_the_clients_kinds() {
        use rubylight_protocol::usb::{UsbReason, UsbState};
        let attach = UsbMessage::AttachRequest {
            request_id: 7,
            vid: 0x0781,
            pid: 0x5581,
            class: 8,
            speed: 3,
            busid: "1-2".into(),
            name: "Flash drive".into(),
        };
        let message = ClientMessage::Usb(attach.clone());
        assert!(message.is_valid());
        assert_eq!(message.announcement(), Some(0x553A));
        assert_eq!(message.delivery(), Delivery::Reliable);
        assert_eq!(UsbMessage::decode(&message.payload()), Some(attach));
        assert!(ClientMessage::Usb(UsbMessage::Detach { request_id: 7 }).is_valid());
        // The host's kinds, and what does not encode, never go out.
        assert!(!ClientMessage::Usb(UsbMessage::TunnelReady {
            request_id: 7,
            token: [1; 16]
        })
        .is_valid());
        assert!(!ClientMessage::Usb(UsbMessage::Status {
            request_id: 7,
            state: UsbState::Attached,
            reason: UsbReason::None
        })
        .is_valid());
        assert!(!ClientMessage::Usb(UsbMessage::Detach { request_id: 0 }).is_valid());
    }
}
