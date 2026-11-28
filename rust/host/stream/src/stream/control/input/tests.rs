//! Regression tests for non-blocking background controller acquisition (`STR-02`).

use super::*;
use std::net::UdpSocket;
use std::sync::mpsc::sync_channel;

fn loopback_wake_pair() -> (UdpSocket, Arc<UdpSocket>) {
    let rx = UdpSocket::bind("127.0.0.1:0").expect("bind loopback wake receiver");
    rx.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let tx = UdpSocket::bind("127.0.0.1:0").expect("bind loopback wake sender");
    tx.connect(rx.local_addr().unwrap())
        .expect("connect loopback wake sender");
    (rx, Arc::new(tx))
}

#[test]
fn async_controller_acquire_does_not_block_caller_holds_pad_input_and_wakes_on_completion() {
    let (wake_rx, wake_tx) = loopback_wake_pair();
    let (started_tx, started_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel(1);
    let mut wait = PadsWait::default();
    let t0 = Instant::now();

    assert!(wait.start_with(t0, wake_tx.clone(), move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
        anyhow::bail!("previous controller still unplugging")
    }));
    // Caller returned immediately while the worker is still blocked.
    assert!(t0.elapsed() < Duration::from_millis(250));
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("background acquire started");
    assert!(wait.is_waiting());
    assert_eq!(wait.retry_at(), None);
    assert!(wait.poll_pending(Instant::now()).is_none());

    // While acquisition is in flight, mouse/keyboard pass through and pad packets are held.
    let arrival = input::Input::Arrival {
        id: 0,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    };
    let controller = input::Input::Controller {
        id: 0,
        active: 1,
        buttons: 1,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let mut inputs = vec![
        input::Input::Relative { x: 5, y: -3 },
        arrival.clone(),
        controller.clone(),
    ];
    wait.hold(&mut inputs);
    assert_eq!(inputs.len(), 1);
    assert!(matches!(inputs[0], input::Input::Relative { x: 5, y: -3 }));

    // Releasing the worker wakes the control loop and schedules a retry timer.
    release_tx.send(()).unwrap();
    let mut byte = [0u8; 1];
    assert_eq!(wake_rx.recv(&mut byte).expect("wake received"), 1);

    let failed_at = Instant::now();
    let polled = wait
        .poll_pending(failed_at)
        .expect("completed result ready");
    assert!(polled.is_err());
    assert_eq!(wait.retry_at(), Some(failed_at + PADS_RETRY));
    assert!(wait.is_waiting());

    // Once retry succeeds, held pad inputs replay in order and waiting clears.
    let replayed = wait
        .retry(failed_at + PADS_RETRY, || Ok(()))
        .expect("retry succeeded");
    assert_eq!(replayed.len(), 2);
    assert!(matches!(replayed[0], input::Input::Arrival { id: 0, .. }));
    assert!(matches!(
        replayed[1],
        input::Input::Controller { id: 0, .. }
    ));
    assert!(!wait.is_waiting());
    assert_eq!(wait.retry_at(), None);
}

#[test]
fn dropping_pads_wait_while_acquire_in_flight_skips_spurious_wake() {
    let (wake_rx, wake_tx) = loopback_wake_pair();
    wake_rx
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let (started_tx, started_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel(1);
    let (done_tx, done_rx) = sync_channel(1);

    {
        let mut wait = PadsWait::default();
        assert!(wait.start_with(Instant::now(), wake_tx, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            let _ = done_tx.send(());
            anyhow::bail!("disconnected")
        }));
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker started");
    }

    release_tx.send(()).unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worker exited cleanly");
    let mut byte = [0u8; 1];
    assert!(
        wake_rx.recv(&mut byte).is_err(),
        "dropped peer must not wake the control loop"
    );
}
