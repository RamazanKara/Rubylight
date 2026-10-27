//! Runs a device whose USB stack only has blocking calls (Android's `UsbDeviceConnection`,
//! libusb's synchronous API): one thread per endpoint, transfers in the order the driver
//! submitted them, as the endpoint itself takes them.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use rubylight_protocol::usbip as wire;

use super::server::{Completion, Direction, Reply, Transfer, UsbDevice};

/// A device that runs one transfer at a time per endpoint, blocking until it ends.
pub trait BlockingDevice: Send + Sync + 'static {
    /// See [`UsbDevice::describe`].
    fn describe(&self) -> wire::Device;
    /// See [`UsbDevice::class`].
    fn class(&self) -> u8 {
        self.describe().class
    }
    /// Runs `transfer` to its end. A transfer that waits for the device (an interrupt IN with
    /// nothing to report) waits in short slices, a second or less, and gives up with
    /// [`wire::ECONNRESET`] once `cancelled` is set.
    fn transfer(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply;
    /// The attachment ended; no transfer runs any more.
    fn close(&self) {}
}

struct Job {
    transfer: Transfer,
    done: Completion,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct Lanes {
    queues: HashMap<(u8, Direction), VecDeque<Job>>,
    /// The cancel flag of each transfer a lane runs now.
    running: HashMap<u32, Arc<AtomicBool>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    closed: bool,
}

struct Inner<D> {
    device: D,
    lanes: Mutex<Lanes>,
    wake: Condvar,
}

impl<D> Inner<D> {
    fn lanes(&self) -> MutexGuard<'_, Lanes> {
        self.lanes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A [`UsbDevice`] made of a [`BlockingDevice`].
pub struct Blocking<D: BlockingDevice> {
    inner: Arc<Inner<D>>,
}

impl<D: BlockingDevice> Blocking<D> {
    pub fn new(device: D) -> Self {
        Self {
            inner: Arc::new(Inner {
                device,
                lanes: Mutex::new(Lanes::default()),
                wake: Condvar::new(),
            }),
        }
    }

    pub fn device(&self) -> &D {
        &self.inner.device
    }
}

/// The control endpoint is one lane both ways; every other endpoint is one per direction.
fn lane_of(transfer: &Transfer) -> (u8, Direction) {
    if transfer.is_control() {
        (0, Direction::Out)
    } else {
        (transfer.endpoint, transfer.direction)
    }
}

fn run_lane<D: BlockingDevice>(inner: Arc<Inner<D>>, lane: (u8, Direction)) {
    loop {
        let job = {
            let mut lanes = inner.lanes();
            loop {
                if lanes.closed {
                    return;
                }
                if let Some(job) = lanes.queues.get_mut(&lane).and_then(VecDeque::pop_front) {
                    lanes
                        .running
                        .insert(job.transfer.seqnum, job.cancelled.clone());
                    break job;
                }
                lanes = inner
                    .wake
                    .wait(lanes)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let reply = inner.device.transfer(&job.transfer, &job.cancelled);
        inner.lanes().running.remove(&job.transfer.seqnum);
        job.done.complete(reply);
    }
}

impl<D: BlockingDevice> UsbDevice for Blocking<D> {
    fn describe(&self) -> wire::Device {
        self.inner.device.describe()
    }

    fn class(&self) -> u8 {
        self.inner.device.class()
    }

    fn submit(&self, transfer: Transfer, done: Completion) {
        let lane = lane_of(&transfer);
        let mut lanes = self.inner.lanes();
        if lanes.closed {
            drop(lanes);
            return done.complete(Reply::failed(wire::ESHUTDOWN));
        }
        if !lanes.queues.contains_key(&lane) {
            let inner = self.inner.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("usb-ep{}-{:?}", lane.0, lane.1))
                .spawn(move || run_lane(inner, lane));
            match spawned {
                Ok(thread) => lanes.threads.push(thread),
                Err(_) => {
                    drop(lanes);
                    return done.complete(Reply::failed(wire::EPROTO));
                }
            }
        }
        lanes.queues.entry(lane).or_default().push_back(Job {
            transfer,
            done,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        drop(lanes);
        self.inner.wake.notify_all();
    }

    fn unlink(&self, seqnum: u32) {
        let mut lanes = self.inner.lanes();
        if let Some(cancelled) = lanes.running.get(&seqnum) {
            cancelled.store(true, Ordering::SeqCst);
            return;
        }
        // Not started: it never runs. The server already dropped it, so its completion
        // writes nothing.
        for queue in lanes.queues.values_mut() {
            queue.retain(|job| job.transfer.seqnum != seqnum);
        }
    }

    fn close(&self) {
        let (threads, queued) = {
            let mut lanes = self.inner.lanes();
            lanes.closed = true;
            for cancelled in lanes.running.values() {
                cancelled.store(true, Ordering::SeqCst);
            }
            let queued: Vec<Job> = lanes
                .queues
                .values_mut()
                .flat_map(|q| q.drain(..))
                .collect();
            (std::mem::take(&mut lanes.threads), queued)
        };
        self.inner.wake.notify_all();
        drop(queued);
        let current = std::thread::current().id();
        for thread in threads {
            // A lane that closes its own device can't wait for itself.
            if thread.thread().id() != current {
                let _ = thread.join();
            }
        }
        self.inner.device.close();
    }
}
