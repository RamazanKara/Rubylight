//! Burst pacing for streams above 120 fps (#144). Two windows were sized for 60 and 120 fps:
//! a conventional-video send batch holds up to 1 ms of wire time
//! ([`network_pacing::batch_payload_limit`]), and the delay-based congestion control groups
//! packets sent within [`BURST_US`] (5 ms) of each other. At 240 Hz a frame lasts 4.2 ms, so one
//! inter-arrival group spans two frames and, at 480 Hz and more, one batch carries most of a
//! frame. Above 120 fps both windows follow the frame period: a batch holds at most a quarter
//! of it, a group at most half. At 120 fps and below every batch and every group is exactly as
//! before, byte for byte.

use std::time::Duration;

use crate::congestion::BURST_US;
use crate::network_pacing;

/// The period of a 120 fps stream as the stream computes it ([`crate::framegen::Rate`]). A
/// shorter period is a stream above 120 fps.
pub const HIGH_HZ_PERIOD: Duration = Duration::from_nanos(1_000_000_000_000 / 120_000);

/// The wire time one conventional-video batch holds at 120 fps and below.
pub const BATCH_WINDOW: Duration = Duration::from_millis(1);

/// Whether a stream with this frame period runs above 120 fps. A zero period (unknown) is not.
pub fn above_120_fps(period: Duration) -> bool {
    !period.is_zero() && period < HIGH_HZ_PERIOD
}

/// The wire time of one batch: [`BATCH_WINDOW`], and above 120 fps no more than a quarter of the
/// frame period, so a frame leaves in at least four paced batches.
pub fn batch_window(period: Duration) -> Duration {
    if above_120_fps(period) {
        BATCH_WINDOW.min(period / 4)
    } else {
        BATCH_WINDOW
    }
}

/// [`network_pacing::batch_payload_limit`] with the batch window of a stream at `period`.
/// Always one datagram at least; never more than the 1 ms limit.
pub fn batch_payload_limit(
    bps: u64,
    datagram: usize,
    ipv6: bool,
    maximum: usize,
    period: Duration,
) -> usize {
    let window = batch_window(period);
    if window == BATCH_WINDOW {
        // The unchanged path: the same function, so not a byte differs.
        return network_pacing::batch_payload_limit(bps, datagram, ipv6, maximum);
    }
    let wire_packet = datagram.saturating_add(network_pacing::overhead(ipv6)) as u64;
    let window_ns = window.as_nanos().min(u128::from(u64::MAX)) as u64;
    let bytes = (u128::from(bps) * u128::from(window_ns) / 8_000_000_000) as u64;
    let packets = (bytes / wire_packet.max(1)).max(1);
    packets
        .saturating_mul(datagram as u64)
        .min(maximum as u64)
        .max(datagram as u64) as usize
}

/// [`network_pacing::startup_batch_payload_limit`] with the batch window of a stream at
/// `period`: the first wire frames keep their start-up cap.
pub fn startup_batch_payload_limit(
    frame: u32,
    bps: u64,
    datagram: usize,
    ipv6: bool,
    maximum: usize,
    period: Duration,
) -> usize {
    let cap = network_pacing::startup_batch_cap(frame, maximum);
    batch_payload_limit(bps, datagram, ipv6, cap, period)
}

/// The congestion control's inter-arrival group window, µs: [`BURST_US`], and above 120 fps no
/// more than half the frame period, so consecutive frames never share a group.
pub fn group_burst_us(period: Duration) -> u64 {
    if above_120_fps(period) {
        let half = (period.as_micros() / 2).min(u128::from(BURST_US)) as u64;
        half.max(1)
    } else {
        BURST_US
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framegen::Rate;

    const DATAGRAMS: [usize; 4] = [576, 1024, 1408, 1452];
    const RATES: [u64; 9] = [
        0,
        1,
        1_000_000,
        20_000_000,
        150_000_000,
        800_000_000,
        2_500_000_000,
        10_000_000_000,
        u64::MAX,
    ];
    const MAXIMA: [usize; 5] = [1, 1024, 4096, 16 * 1024, 64 * 1024];
    const FRAMES: [u32; 6] = [0, 1, 7, 8, 9, 100_000];

    fn period(millihz: u32) -> Duration {
        Rate(millihz).period()
    }

    /// Every rate a stream may run at up to 120 fps, as the stream and as a float compute it.
    fn periods_up_to_120() -> Vec<Duration> {
        let mut periods: Vec<Duration> = [
            1_000, 24_000, 30_000, 50_000, 59_940, 60_000, 72_000, 90_000, 100_000, 119_880,
            120_000,
        ]
        .into_iter()
        .map(period)
        .collect();
        periods.push(Duration::from_secs_f64(1. / 120.));
        periods.push(Duration::ZERO);
        periods.push(Duration::from_secs(1));
        periods
    }

    #[test]
    fn at_120_fps_and_below_every_batch_is_as_before() {
        for period in periods_up_to_120() {
            assert!(!above_120_fps(period), "{period:?}");
            assert_eq!(batch_window(period), BATCH_WINDOW);
            for bps in RATES {
                for datagram in DATAGRAMS {
                    for ipv6 in [false, true] {
                        for maximum in MAXIMA {
                            assert_eq!(
                                batch_payload_limit(bps, datagram, ipv6, maximum, period),
                                network_pacing::batch_payload_limit(bps, datagram, ipv6, maximum),
                                "{period:?} {bps} {datagram} {ipv6} {maximum}"
                            );
                            for frame in FRAMES {
                                assert_eq!(
                                    startup_batch_payload_limit(
                                        frame, bps, datagram, ipv6, maximum, period
                                    ),
                                    network_pacing::startup_batch_payload_limit(
                                        frame, bps, datagram, ipv6, maximum
                                    ),
                                    "{period:?} {frame} {bps} {datagram} {ipv6} {maximum}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn at_120_fps_and_below_every_group_is_as_before() {
        for period in periods_up_to_120() {
            assert_eq!(group_burst_us(period), BURST_US, "{period:?}");
        }
    }

    #[test]
    fn above_120_fps_is_any_faster_rate() {
        for millihz in [
            120_001, 121_000, 144_000, 165_000, 240_000, 480_000, 1_000_000,
        ] {
            assert!(above_120_fps(period(millihz)), "{millihz}");
        }
    }

    #[test]
    fn a_batch_holds_a_quarter_frame_once_that_is_under_a_millisecond() {
        // 144 and 240 Hz: a quarter frame is still more than 1 ms, so the batch stays.
        assert_eq!(batch_window(period(144_000)), BATCH_WINDOW);
        assert_eq!(batch_window(period(240_000)), BATCH_WINDOW);
        assert_eq!(
            batch_window(period(480_000)),
            Duration::from_nanos(2_083_333 / 4)
        );
        assert_eq!(batch_window(period(1_000_000)), Duration::from_micros(250));
        // 200 Mbps of 1408-byte datagrams over IPv4 (1474 wire bytes): 16 per millisecond,
        // 8 per 520.8 µs at 480 Hz and 4 per 250 µs at 1000 Hz.
        let limit =
            |millihz| batch_payload_limit(200_000_000, 1408, false, 1 << 20, period(millihz));
        assert_eq!(limit(120_000), 16 * 1408);
        assert_eq!(limit(240_000), 16 * 1408);
        assert_eq!(limit(480_000), 8 * 1408);
        assert_eq!(limit(1_000_000), 4 * 1408);
    }

    #[test]
    fn a_faster_stream_never_sends_a_bigger_batch() {
        for millihz in [
            121_000, 144_000, 240_000, 360_000, 480_000, 500_000, 1_000_000,
        ] {
            for bps in RATES {
                for datagram in DATAGRAMS {
                    for maximum in MAXIMA {
                        let scaled =
                            batch_payload_limit(bps, datagram, true, maximum, period(millihz));
                        let fixed =
                            network_pacing::batch_payload_limit(bps, datagram, true, maximum);
                        assert!(
                            scaled <= fixed.max(datagram),
                            "{millihz} {bps} {datagram} {maximum}"
                        );
                        assert!(scaled >= datagram, "one datagram at least");
                    }
                }
            }
        }
    }

    #[test]
    fn the_first_frames_keep_their_start_up_cap_at_high_rates() {
        let at = |frame| {
            startup_batch_payload_limit(
                frame,
                10_000_000_000,
                1408,
                false,
                64 * 1024,
                period(480_000),
            )
        };
        assert!(at(1) <= network_pacing::STARTUP_BATCH_CAP_BYTES);
        assert_eq!(at(0), at(network_pacing::STARTUP_BATCH_FRAMES + 1));
    }

    #[test]
    fn consecutive_frames_never_share_a_group_above_120_fps() {
        assert_eq!(group_burst_us(period(144_000)), 3_472);
        assert_eq!(group_burst_us(period(240_000)), 2_083);
        assert_eq!(group_burst_us(period(480_000)), 1_041);
        assert_eq!(group_burst_us(period(1_000_000)), 500);
        for millihz in [
            120_001, 144_000, 165_000, 240_000, 360_000, 480_000, 1_000_000,
        ] {
            let p = period(millihz);
            let burst = group_burst_us(p);
            assert!(burst < p.as_micros() as u64, "{millihz}");
            assert!(burst <= BURST_US);
        }
    }
}
