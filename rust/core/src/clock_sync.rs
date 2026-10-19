//! Clock sync (control message 0x5537): the client probes, the host answers at once with the
//! times it received the probe and sent the answer, on the video stream's RTP clock. The
//! client maps frame timestamps to its own clock with them, for VRR timestamp presentation
//! and end-to-end latency (`rubylight_protocol::present_clock`).
pub use rubylight_protocol::control::{
    CLOCK_SYNC_MESSAGE_TYPE, CLOCK_SYNC_REPLY_BYTES, ClockSyncReply, ClockSyncRequest,
};
use std::time::Instant;

/// Microseconds on the RTP clock that starts at `epoch`.
fn micros(epoch: Instant, at: Instant) -> u64 {
    at.saturating_duration_since(epoch).as_micros() as u64
}

/// The reply to `payload`, received at `received` and sent now. `None` for a malformed
/// probe, or while no video clock runs (`epoch` unset): the client then keeps probing.
pub fn reply(
    epoch: Option<Instant>,
    received: Instant,
    payload: &[u8],
) -> Option<[u8; CLOCK_SYNC_REPLY_BYTES]> {
    let epoch = epoch?;
    let request = ClockSyncRequest::decode(payload)?;
    let host_receive_us = micros(epoch, received);
    Some(
        ClockSyncReply {
            sequence: request.sequence,
            client_us: request.client_us,
            host_receive_us,
            host_send_us: micros(epoch, Instant::now()).max(host_receive_us),
        }
        .encode(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn replies_on_the_rtp_clock_once_video_runs() {
        let probe = ClockSyncRequest {
            sequence: 9,
            client_us: 123_456,
        }
        .encode();
        let received = Instant::now();
        assert_eq!(reply(None, received, &probe), None, "no video clock yet");
        assert_eq!(reply(Some(received), received, &probe[..15]), None);
        let epoch = received - Duration::from_millis(1500);
        let answer =
            ClockSyncReply::decode(&reply(Some(epoch), received, &probe).unwrap()).unwrap();
        assert_eq!(answer.sequence, 9);
        assert_eq!(answer.client_us, 123_456);
        assert_eq!(answer.host_receive_us, 1_500_000);
        assert!(answer.host_send_us >= answer.host_receive_us);
        assert!(answer.host_send_us < 1_500_000 + 1_000_000);
        // The RTP timestamp of a frame stamped at the receive time.
        assert_eq!((answer.host_receive_us * 9 / 100) as u32, 135_000);
    }
}
