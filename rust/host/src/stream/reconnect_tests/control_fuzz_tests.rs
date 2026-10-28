//! Arbitrary control messages, in the clear and encrypted, into a live stream: the stream
//! keeps running and ends as any other (`Harness::end`). Kinds that reach this PC's own
//! input, keyboard layout, USB, displays, clipboard or server commands are left out: the
//! test host is a real PC.

use super::*;
use butterpollo_core::{
    clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE, keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE,
    rate_change::RATE_CHANGE_MESSAGE_TYPE, reconfigure::RECONFIGURE_MESSAGE_TYPE,
    text_input::TEXT_INPUT_MESSAGE_TYPE, usb_forward::USB_MESSAGE_TYPE,
};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;

/// Kinds with effects outside the stream.
const OUTSIDE: [u16; 11] = [
    0x0109, // termination
    0x0206, // input
    0x3000, // server command
    KEYBOARD_LAYOUT_MESSAGE_TYPE,
    USB_MESSAGE_TYPE,
    RECONFIGURE_MESSAGE_TYPE,
    RATE_CHANGE_MESSAGE_TYPE,
    TEXT_INPUT_MESSAGE_TYPE,
    CLIPBOARD_SYNC_MESSAGE_TYPE,
    controller_audio::CONTROL,
    controller_audio::MICROPHONE,
];

#[derive(Debug, Clone)]
enum Message {
    Plain(u16, Vec<u8>),
    Encrypted(u32, u16, Vec<u8>),
}

fn messages() -> impl Strategy<Value = Vec<Message>> {
    use butterpollo_core::{
        client_cursor, clock_sync, display_caps, host_timing, link_notice, phase_sync,
        pipeline_rates, statistical_fec,
    };
    use proptest::prelude::*;
    let kind = prop_oneof![
        3 => proptest::sample::select(vec![
            0x0200,
            0x0301,
            0x0302,
            0x5502,
            statistical_fec::FEC_REPORT_MESSAGE_TYPE,
            phase_sync::REPORT_MESSAGE_TYPE,
            clock_sync::CLOCK_SYNC_MESSAGE_TYPE,
            host_timing::HOST_TIMING_MESSAGE_TYPE,
            link_notice::LINK_NOTICE_MESSAGE_TYPE,
            pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE,
            client_cursor::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE,
            display_caps::DISPLAY_CAPS_MESSAGE_TYPE,
        ]),
        1 => any::<u16>(),
    ]
    .prop_filter("an effect outside the stream", |kind| {
        !OUTSIDE.contains(kind)
    });
    let payload = || {
        prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..16),
            proptest::collection::vec(any::<u8>(), 0..300),
        ]
    };
    let message = prop_oneof![
        (kind.clone(), payload()).prop_map(|(k, p)| Message::Plain(k, p)),
        (0u32..64, kind, payload()).prop_map(|(seq, k, p)| Message::Encrypted(seq, k, p)),
    ];
    proptest::collection::vec(message, 200..=200)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arbitrary_control_messages_leave_the_stream_running() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let mut runner = TestRunner::deterministic();
    for _ in 0..4 {
        let batch = messages().new_tree(&mut runner).unwrap().current();
        for message in batch {
            match message {
                Message::Plain(kind, payload) => client.send_control(kind, &payload),
                Message::Encrypted(seq, kind, payload) => {
                    client.send_encrypted(&launch.key, seq, kind, &payload)
                }
            }
        }
        client.quiet_heartbeat().await;
        assert!(!session.stopping(), "{}", session.termination_reason());
    }
    h.counts(1, 0).await?;
    h.end(&session, client).await?;
    h.finish().await
}
