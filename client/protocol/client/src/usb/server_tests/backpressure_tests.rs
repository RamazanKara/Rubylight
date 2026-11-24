//! Writer backpressure and full-queue tests for the USB/IP server.

use super::*;

/// A tunnel whose writes wait while `held`, as a PC that stops reading leaves them.
struct Held {
    inner: Arc<TcpTunnel>,
    held: Mutex<bool>,
    released: std::sync::Condvar,
    waiting: AtomicUsize,
    closed: AtomicBool,
}
impl Tunnel for Held {
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(bytes)
    }
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut held = self.held.lock().unwrap();
        if *held {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            while *held && !self.closed.load(Ordering::SeqCst) {
                held = self.released.wait(held).unwrap();
            }
        }
        drop(held);
        if self.closed.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.inner.write_all(bytes)
    }
    fn close(&self) {
        // A closed connection fails a held write, as a shutdown fails a blocked one.
        self.closed.store(true, Ordering::SeqCst);
        self.inner.close();
        let _held = self.held.lock().unwrap();
        self.released.notify_all();
    }
}

impl Held {
    fn new(inner: Arc<TcpTunnel>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            held: Mutex::new(false),
            released: std::sync::Condvar::new(),
            waiting: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        })
    }

    fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }

    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.released.notify_all();
    }
}

/// Waits up to 3 s for `done`.
fn within(done: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    while !done() {
        if started.elapsed() > Duration::from_secs(3) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[test]
fn an_unlink_answered_while_the_pc_reads_nothing_does_not_stop_the_serve_loop() {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(within(|| device.kept.lock().unwrap().len() == 1));
    // Nothing waits to go out; the PC stops reading, then gives up on the transfer.
    tunnel.hold();
    let unlink = driver.unlink(first);
    assert!(
        within(|| tunnel.waiting.load(Ordering::SeqCst) > 0),
        "the unlink was never answered"
    );
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = within(|| device.kept.lock().unwrap().len() == 2);
    tunnel.release();
    assert!(took, "the serve loop waited behind its own answer");
    assert_eq!(status_of(&driver.reply(false).0), (unlink, wire::ECONNRESET));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn a_completion_made_inside_submit_while_the_pc_reads_nothing_does_not_stop_the_serve_loop() {
    /// Completes every transfer inside `submit`, on the serve loop's thread.
    #[derive(Default)]
    struct AtOnce {
        submitted: AtomicUsize,
    }
    impl UsbDevice for AtOnce {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn submit(&self, _: Transfer, done: Completion) {
            self.submitted.fetch_add(1, Ordering::SeqCst);
            done.complete(Reply::read(vec![3]));
        }
        fn unlink(&self, _: u32) {}
        fn close(&self) {}
    }
    let device = Arc::new(AtOnce::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    tunnel.hold();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(
        within(|| tunnel.waiting.load(Ordering::SeqCst) > 0),
        "the completion was never written"
    );
    let second = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = within(|| device.submitted.load(Ordering::SeqCst) == 2);
    tunnel.release();
    assert!(took, "the serve loop waited behind a completion it made");
    for seqnum in [first, second] {
        let (header, data, _) = driver.reply(true);
        assert_eq!((status_of(&header), data), ((seqnum, 0), vec![3]));
    }
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn a_completion_whose_write_waits_on_the_pc_does_not_stop_the_serve_loop() {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let second = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let kept = |count: usize| {
        let started = std::time::Instant::now();
        while device.kept.lock().unwrap().len() < count {
            if started.elapsed() > Duration::from_secs(3) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    };
    assert!(kept(2));
    // The PC stops reading: the first completion's write waits.
    *tunnel.held.lock().unwrap() = true;
    let completion = device.kept.lock().unwrap().remove(0);
    assert_eq!(completion.seqnum(), first);
    let completing = std::thread::spawn(move || completion.complete(Reply::read(vec![1])));
    while tunnel.waiting.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // The serve loop still takes a submit, an unlink and a submit after it.
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let unlink = driver.unlink(second);
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = kept(3);
    *tunnel.held.lock().unwrap() = false;
    tunnel.released.notify_all();
    assert!(took, "the serve loop waited behind the blocked write");
    completing.join().unwrap();
    // The unlink's answer comes after the completion it followed.
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((first, 0), vec![1]));
    assert_eq!(status_of(&driver.reply(false).0), (unlink, wire::ECONNRESET));
    drop(driver);
    served.join().unwrap().unwrap();
}

/// A served device whose PC reads nothing, with [`MAX_IN_FLIGHT`] answers waiting for it and the
/// serve loop waiting for room to answer an unlink. Returns the transfer still in flight.
type Served = std::thread::JoinHandle<Result<(), ServeError>>;

fn full_queue() -> (Arc<Held>, Driver, Served, Completion) {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    tunnel.hold();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    for _ in 1..MAX_IN_FLIGHT {
        driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    }
    assert!(within(|| device.kept.lock().unwrap().len() == MAX_IN_FLIGHT));
    let mut kept = std::mem::take(&mut *device.kept.lock().unwrap());
    // The writer takes the first answer and waits in its write; the rest queue behind it.
    kept.remove(0).complete(Reply::read(vec![1]));
    assert!(within(|| tunnel.waiting.load(Ordering::SeqCst) > 0));
    for done in kept {
        done.complete(Reply::read(vec![1]));
    }
    // One more is taken, the next refused: its ENOMEM fills the queue.
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(within(|| device.kept.lock().unwrap().len() == 1));
    let in_flight = device.kept.lock().unwrap().remove(0);
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    // An unlink of a transfer already answered: its answer would go past the queue's room,
    // so the serve loop waits. Only that unlink can make it wait: the refusal before it fit.
    driver.unlink(first);
    assert!(
        within(|| in_flight.shared.state().waiting_for_room),
        "the serve loop never waited for room"
    );
    (tunnel, driver, served, in_flight)
}

/// Waits up to 3 s for `served` to end.
fn ended(served: &Served) -> bool {
    within(|| served.is_finished())
}

#[test]
fn a_tunnel_closed_while_the_serve_loop_waits_for_room_ends_serving() {
    let (tunnel, _driver, served, in_flight) = full_queue();
    assert!(!served.is_finished(), "the serve loop does not wait for room");
    tunnel.close();
    let ended = ended(&served);
    drop(in_flight);
    assert!(ended, "serve never returned");
    assert!(matches!(served.join().unwrap(), Err(ServeError::Io(_))));
}

#[test]
fn a_bad_reply_while_the_serve_loop_waits_for_room_ends_serving() {
    let (tunnel, _driver, served, in_flight) = full_queue();
    assert!(!served.is_finished(), "the serve loop does not wait for room");
    // More data than the 8 bytes asked fails serving and closes the tunnel.
    in_flight.complete(Reply::read(vec![0; 9]));
    let ended = ended(&served);
    tunnel.close();
    assert!(ended, "serve never returned");
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
}
