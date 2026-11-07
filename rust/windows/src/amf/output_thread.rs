//! `amd_output_thread`: a thread of its own waits in AMF's QueryOutput and
//! queues what it returns, so the stream thread takes finished frames without
//! waiting in the driver. AMF allows submission and output on separate
//! threads, as its own samples do. Everything a returned buffer means (frame
//! bookkeeping, re-encoding a cut as a keyframe, header repair, frame caps)
//! stays on the stream thread: this thread only queries.
use super::*;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use windows::Win32::System::Threading::WaitForSingleObject;

/// A query that comes back this fast with nothing did not wait in the
/// driver (some ignore QueryTimeout); the thread then pauses as the stream
/// thread's own output poll does, instead of spinning.
const UNWAITED: Duration = Duration::from_micros(200);
const PAUSE: Duration = Duration::from_micros(100);
/// How long an idle thread sleeps between checks; a submission wakes it.
const IDLE: Duration = Duration::from_millis(5);

struct Shared {
    /// Each result with its buffer, whose reference the queue holds.
    queue: Mutex<VecDeque<(AMF_RESULT, usize)>>,
    /// Set while the queue holds output.
    ready: crate::timing::Signal,
    /// Frames are in flight: query them.
    wanted: AtomicBool,
    stop: AtomicBool,
}

/// The component, handed to the thread.
struct Component(*mut AMFComponent);
// SAFETY: AMF lets one thread query a component while another submits to it;
// the encoder joins this thread before it terminates and releases the
// component, and nothing else crosses threads.
unsafe impl Send for Component {}

pub(super) struct OutputThread {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OutputThread {
    /// # Safety
    /// `component` is initialized and stays live until this is dropped.
    pub(super) unsafe fn for_component(component: *mut AMFComponent) -> Result<Self> {
        let component = Component(component);
        Self::start(move |data| {
            let component = &component;
            // SAFETY: Per this function's contract the component is live;
            // QueryOutput hands over one reference to the buffer, which the
            // queue keeps.
            timed("AMF QueryOutput", || unsafe {
                ((*(*component.0).pVtbl).QueryOutput.unwrap())(component.0, data)
            })
        })
    }
    /// The thread, querying with `query`.
    fn start(
        mut query: impl FnMut(*mut *mut AMFData) -> AMF_RESULT + Send + 'static,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            ready: crate::timing::Signal::new()?,
            wanted: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("amf-output".into())
            .spawn(move || {
                let _priority = crate::capture::Priority::new();
                let timer = crate::timing::Timer::new().ok();
                while !worker.stop.load(Ordering::Acquire) {
                    if !worker.wanted.load(Ordering::Acquire) {
                        std::thread::park_timeout(IDLE);
                        continue;
                    }
                    let started = Instant::now();
                    let mut data = ptr::null_mut();
                    let code = query(&mut data);
                    let nothing = data.is_null()
                        && matches!(
                            code,
                            AMF_RESULT_AMF_OK
                                | AMF_RESULT_AMF_REPEAT
                                | AMF_RESULT_AMF_EOF
                                | AMF_RESULT_AMF_NEED_MORE_INPUT
                        );
                    if nothing {
                        if started.elapsed() < UNWAITED {
                            match &timer {
                                Some(timer) => timer.until(Instant::now() + PAUSE),
                                None => std::thread::sleep(PAUSE),
                            }
                        }
                        continue;
                    }
                    worker
                        .queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push_back((code, data as usize));
                    let _ = worker.ready.set();
                    if code != AMF_RESULT_AMF_OK {
                        // The stream thread reports the failure and recreates
                        // the encoder; querying further gains nothing.
                        break;
                    }
                }
            })
            .context("starting the AMF output thread")?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    /// Frames are (or are no longer) in flight. A submission wakes an idle
    /// thread at once.
    pub(super) fn want(&self, wanted: bool) {
        let was = self.shared.wanted.swap(wanted, Ordering::AcqRel);
        if wanted
            && !was
            && let Some(thread) = &self.thread
        {
            thread.thread().unpark();
        }
    }
    /// The oldest result the thread queued: its code and buffer (null for a
    /// failure), or `None` while there is none.
    pub(super) fn take(&self) -> Option<(AMF_RESULT, *mut AMFData)> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .map(|(code, data)| (code, data as *mut AMFData))
    }
    /// The event a wait for output adds: set while output is queued, reset
    /// otherwise so that it ends the wait with the next one. Checked under
    /// the queue's lock, so a result queued meanwhile still sets it.
    pub(super) fn event(&self) -> &crate::timing::Signal {
        let queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _ = if queue.is_empty() {
            self.shared.ready.reset()
        } else {
            self.shared.ready.set()
        };
        &self.shared.ready
    }
    /// Wait up to `timeout` (rounded up to whole milliseconds) for output.
    pub(super) fn wait(&self, timeout: Duration) {
        let event = self.event();
        let ms = timeout
            .as_nanos()
            .div_ceil(1_000_000)
            .min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: The event is open while `self` is borrowed.
        unsafe {
            WaitForSingleObject(event.raw(), ms);
        }
    }
}

impl Drop for OutputThread {
    /// Before the encoder terminates the component: stop and join the
    /// thread (its query returns within the query timeout), then release
    /// the buffers nobody took.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::error!("the AMF output thread panicked");
            }
        }
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (_, data) in queue.drain(..) {
            let data = data as *mut AMFData;
            if !data.is_null() {
                // SAFETY: The queue holds the one reference QueryOutput handed over.
                unsafe { ((*(*data).pVtbl).Release.unwrap())(data) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A frame's buffer; never dereferenced, as the test takes it first.
    const FRAME: usize = 0x10;

    #[test]
    fn the_thread_queries_only_while_wanted_and_queues_frames_and_failures() -> Result<()> {
        let (answers, script) = mpsc::channel::<(AMF_RESULT, usize)>();
        let queried = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = queried.clone();
        let thread = OutputThread::start(move |data| {
            count.fetch_add(1, Ordering::SeqCst);
            match script.recv_timeout(Duration::from_millis(1)) {
                Ok((code, buffer)) => {
                    // SAFETY: `data` is the thread's own out pointer.
                    unsafe { *data = buffer as *mut AMFData };
                    code
                }
                Err(_) => AMF_RESULT_AMF_REPEAT,
            }
        })?;
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(queried.load(Ordering::SeqCst), 0, "idle until wanted");
        assert!(thread.take().is_none());
        thread.want(true);
        answers.send((AMF_RESULT_AMF_OK, FRAME)).unwrap();
        thread.wait(Duration::from_secs(2));
        assert_eq!(
            thread.take(),
            Some((AMF_RESULT_AMF_OK, FRAME as *mut AMFData))
        );
        // An empty queue resets the event: a wait now runs to its timeout.
        let started = Instant::now();
        thread.wait(Duration::from_millis(5));
        assert!(started.elapsed() >= Duration::from_millis(4));
        // A failure is queued and ends the thread's querying.
        answers.send((AMF_RESULT_AMF_FAIL, 0)).unwrap();
        thread.wait(Duration::from_secs(2));
        assert_eq!(thread.take(), Some((AMF_RESULT_AMF_FAIL, ptr::null_mut())));
        let after = queried.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(
            queried.load(Ordering::SeqCst),
            after,
            "stopped after a failure"
        );
        drop(thread);
        Ok(())
    }
}
