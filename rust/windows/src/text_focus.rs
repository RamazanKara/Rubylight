//! The focused text field for keyboard follows focus (0x553B, Experimental): a thread that
//! owns UI Automation's focus-changed handler and reports each text field the focus moves
//! to, or that it left them. With `max_text` above 0 it also follows the field's text and
//! selection and reads up to that much around the cursor, never in a password field.
//!
//! UI Automation calls the handlers on its own threads. They only wake the watcher, which
//! reads the newest focus itself, so a burst of changes costs one read and nothing here
//! runs on the video or input paths. A focus event that leaves the focus where it was (a
//! tooltip, the host's own window, an app too busy to answer) reports nothing, so the
//! keyboard does not lower and raise again for it.

use anyhow::{Context, Result};
use butterpollo_core::text_input::{self, Field, Focused, FocusedElement, Surrounding, Watch};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::{LPARAM, WPARAM},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Ole::{SafeArrayAccessData, SafeArrayDestroy, SafeArrayUnaccessData},
            Threading::GetCurrentThreadId,
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
                IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
                IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
                IUIAutomationTextPattern, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
                TextPatternRangeEndpoint_Start, TextUnit_Character, TreeScope_Element,
                UIA_AriaRolePropertyId, UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId,
                UIA_ControlTypePropertyId, UIA_EVENT_ID, UIA_IsEnabledPropertyId,
                UIA_IsPasswordPropertyId, UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId,
                UIA_ProcessIdPropertyId, UIA_RuntimeIdPropertyId, UIA_Text_TextChangedEventId,
                UIA_Text_TextSelectionChangedEventId, UIA_TextPatternId,
                UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId,
            },
            WindowsAndMessaging::{
                DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
                PostThreadMessageW, WM_APP, WM_QUIT,
            },
        },
    },
    core::{BSTR, HRESULT, Ref, implement},
};

/// The focus moved.
const WAKE_FOCUS: u32 = WM_APP + 1;
/// The followed field's text or selection changed.
const WAKE_TEXT: u32 = WM_APP + 2;
/// The client asked for another watch ([`FocusWatch::set`]).
const WAKE_SET: u32 = WM_APP + 3;
/// The text events followed in a field whose text is read.
const TEXT_EVENTS: [UIA_EVENT_ID; 2] = [
    UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
];
/// How long a new watcher waits for a stopped one still stuck in UI Automation.
const LINGER_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

/// Stopped watchers whose thread has not ended yet, so a hung app costs at most one
/// thread: the next watcher waits for it, then fails.
static LINGERING: Mutex<usize> = Mutex::new(0);
static ENDED: Condvar = Condvar::new();
#[cfg(test)]
static SPAWNED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Watches the focus until dropped, one thread for the watcher's whole life: the client's
/// toggles pause and resume it ([`FocusWatch::set`]). Neither starting, setting nor
/// dropping waits for the watcher's thread: the control loop drives it, and a UI
/// Automation call stuck in a hung app must not hold up input.
pub struct FocusWatch {
    control: Arc<Control>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// How the watcher is told what to do: its thread id once it has a message queue, the
/// newest watch and whether it stopped, under one lock that also covers each report.
#[derive(Default)]
struct Control {
    state: Mutex<State>,
    /// Whether the thread ended, and whether it was dropped before, under [`LINGERING`].
    ended: AtomicBool,
    detached: AtomicBool,
}

struct State {
    thread: u32,
    stop: bool,
    watch: Watch,
}

impl Default for State {
    fn default() -> Self {
        Self {
            thread: 0,
            stop: false,
            watch: Watch::Off,
        }
    }
}

impl Control {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FocusWatch {
    /// Calls `report` on the watcher's thread with the focused text field, or `None` once the
    /// focus left text fields, first for the focus at start. `max_text` above 0 also reads up
    /// to that many bytes around the cursor of a field that is not a password, and reports
    /// again when its text or selection changes. `failed` gets why UI Automation did not
    /// start or follow the focus again, on the same thread; `Err` only when there is no
    /// thread.
    pub fn start(
        max_text: usize,
        report: impl Fn(Option<Focused>) + Send + 'static,
        failed: impl Fn(anyhow::Error) + Send + 'static,
    ) -> Result<Self> {
        let control = Arc::new(Control::default());
        control.state().watch = Watch::On { max_text };
        let shared = control.clone();
        let worker = std::thread::Builder::new()
            .name("text-focus".into())
            .spawn(move || {
                let _ended = Ended(&shared);
                if let Err(error) = wait_for_lingering(&shared).and_then(|()| {
                    // SAFETY: on this new thread, which owns the watcher's COM apartment.
                    unsafe { run(&report, &failed, &shared) }
                }) {
                    failed(error);
                }
            })
            .context("the focus watcher thread did not start")?;
        #[cfg(test)]
        SPAWNED.fetch_add(1, Ordering::SeqCst);
        Ok(Self {
            control,
            worker: Some(worker),
        })
    }

    /// Pauses the watcher for [`Watch::Off`], which reports nothing more once this returns,
    /// or resumes it, reading as much text as `On` says, and reports the focus again.
    pub fn set(&self, watch: Watch) {
        let mut state = self.control.state();
        state.watch = watch;
        // The thread reads the watch after it stores its id, under the same lock.
        if state.thread != 0 {
            post(state.thread, WAKE_SET);
        }
    }

    /// Stops the watcher and waits for its thread.
    #[cfg(test)]
    fn stop_and_wait(mut self) {
        let worker = self.worker.take();
        drop(self);
        worker.unwrap().join().unwrap();
    }
}

impl Drop for FocusWatch {
    fn drop(&mut self) {
        {
            // A report in progress finishes first; none comes after.
            let mut state = self.control.state();
            state.stop = true;
            if state.thread != 0 {
                post(state.thread, WM_QUIT);
            }
        }
        let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.control.ended.load(Ordering::SeqCst) {
            self.control.detached.store(true, Ordering::SeqCst);
            *lingering += 1;
        }
        // Detached: it ends once UI Automation returns.
        drop(self.worker.take());
    }
}

/// Marks the watcher's thread ended, also when it panics.
struct Ended<'a>(&'a Control);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
        self.0.ended.store(true, Ordering::SeqCst);
        if self.0.detached.load(Ordering::SeqCst) {
            *lingering -= 1;
            ENDED.notify_all();
        }
    }
}

/// Waits up to [`LINGER_WAIT`] for stopped watchers to end, so they never pile up behind a
/// hung app.
fn wait_for_lingering(control: &Control) -> Result<()> {
    let deadline = Instant::now() + LINGER_WAIT;
    let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
    while *lingering > 0 && !control.state().stop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            anyhow::bail!("a focus watcher before this one is still stuck in UI Automation");
        }
        // Woken when one ends; a stop of this one is seen within the step.
        lingering = ENDED
            .wait_timeout(lingering, left.min(Duration::from_millis(50)))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    Ok(())
}

fn post(thread: u32, message: u32) {
    // SAFETY: PostThreadMessageW only takes a thread id and plain values; a stale id fails.
    unsafe {
        let _ = PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0));
    }
}

unsafe fn run(
    report: &dyn Fn(Option<Focused>),
    failed: &dyn Fn(anyhow::Error),
    control: &Control,
) -> Result<()> {
    // SAFETY: COM is initialized and uninitialized on this thread, every UI Automation object
    // is created, used and released on it before CoUninitialize, and `message` is a live local
    // for every message call.
    unsafe {
        let mut message = MSG::default();
        // The thread needs a message queue before anything posts to it.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        {
            let mut state = control.state();
            state.thread = GetCurrentThreadId();
            if state.stop {
                return Ok(());
            }
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("COM did not start for the focus watcher")?;
        // Checked under the lock Drop and set take: once they return, a watcher that was
        // stopped or paused while it read the focus reports nothing, so it never reports
        // after the one that replaced it or after the client turned it off.
        let report = |focused: Option<Focused>| {
            let state = control.state();
            if !state.stop && state.watch != Watch::Off {
                report(focused);
            }
        };
        let report: &dyn Fn(Option<Focused>) = &report;
        let result = Watcher::new(GetCurrentThreadId()).map(|mut watcher| {
            let watch = control.state().watch;
            if let Err(error) = watcher.apply(watch, report) {
                failed(error);
            }
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let (mut focus, mut text, mut set) = (false, false, false);
                loop {
                    match message.message {
                        WAKE_FOCUS => focus = true,
                        WAKE_TEXT => text = true,
                        WAKE_SET => set = true,
                        // COM's own window in this apartment.
                        _ => {
                            DispatchMessageW(&message);
                        }
                    }
                    if !PeekMessageW(&mut message, None, WAKE_FOCUS, WAKE_SET, PM_REMOVE).as_bool()
                    {
                        break;
                    }
                }
                if set {
                    let watch = control.state().watch;
                    if let Err(error) = watcher.apply(watch, report) {
                        failed(error);
                    }
                } else if focus && watcher.registered {
                    watcher.focus_moved(report);
                } else if text {
                    watcher.text_changed(report);
                }
            }
            watcher.stop();
        });
        CoUninitialize();
        result
    }
}

/// Wakes the watcher thread from UI Automation's.
#[implement(IUIAutomationFocusChangedEventHandler, IUIAutomationEventHandler)]
struct Wake(u32);

impl IUIAutomationFocusChangedEventHandler_Impl for Wake_Impl {
    fn HandleFocusChangedEvent(
        &self,
        _sender: Ref<IUIAutomationElement>,
    ) -> windows::core::Result<()> {
        post(self.0, WAKE_FOCUS);
        Ok(())
    }
}

impl IUIAutomationEventHandler_Impl for Wake_Impl {
    fn HandleAutomationEvent(
        &self,
        _sender: Ref<IUIAutomationElement>,
        _event: UIA_EVENT_ID,
    ) -> windows::core::Result<()> {
        post(self.0, WAKE_TEXT);
        Ok(())
    }
}

struct Watcher {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    focus_handler: IUIAutomationFocusChangedEventHandler,
    text_handler: IUIAutomationEventHandler,
    max_text: usize,
    /// The focused text field.
    field: Option<(IUIAutomationElement, Field)>,
    /// The focused text field's id ([`Focused::element`]), from its runtime id each time the
    /// focus moves to another one.
    element: u32,
    /// The field whose text events are followed.
    followed: Option<IUIAutomationElement>,
    /// Whether the focus handler is registered: while the watch is on.
    registered: bool,
}

impl Watcher {
    unsafe fn new(thread: u32) -> Result<Self> {
        // SAFETY: the caller initialized COM on this thread.
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    .context("UI Automation is not available")?;
            let cache = automation.CreateCacheRequest()?;
            for property in [
                UIA_ControlTypePropertyId,
                UIA_IsEnabledPropertyId,
                UIA_IsPasswordPropertyId,
                UIA_ValueIsReadOnlyPropertyId,
                UIA_AriaRolePropertyId,
                UIA_NamePropertyId,
                UIA_AutomationIdPropertyId,
                UIA_ClassNamePropertyId,
                UIA_LocalizedControlTypePropertyId,
                UIA_RuntimeIdPropertyId,
                UIA_ProcessIdPropertyId,
            ] {
                cache.AddProperty(property)?;
            }
            cache.AddPattern(UIA_ValuePatternId)?;
            cache.AddPattern(UIA_TextPatternId)?;
            let wake = Wake(thread);
            let focus_handler: IUIAutomationFocusChangedEventHandler = wake.into();
            let text_handler: IUIAutomationEventHandler =
                windows::core::Interface::cast(&focus_handler)?;
            Ok(Self {
                automation,
                cache,
                focus_handler,
                text_handler,
                max_text: 0,
                field: None,
                element: 0,
                followed: None,
                registered: false,
            })
        }
    }

    /// Pauses for [`Watch::Off`]: no handlers and no field, so the next `On` reads the id of
    /// the field it finds again. `On` follows the focus again and reports it.
    unsafe fn apply(&mut self, watch: Watch, report: &dyn Fn(Option<Focused>)) -> Result<()> {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            match watch {
                Watch::Off => {
                    self.follow(None);
                    if self.registered {
                        self.registered = false;
                        let _ = self
                            .automation
                            .RemoveFocusChangedEventHandler(&self.focus_handler);
                    }
                    self.field = None;
                }
                Watch::On { max_text } => {
                    self.max_text = max_text;
                    if !self.registered {
                        self.automation
                            .AddFocusChangedEventHandler(&self.cache, &self.focus_handler)
                            .context("UI Automation does not report focus changes")?;
                        self.registered = true;
                    }
                    self.focus_moved(report);
                }
            }
            Ok(())
        }
    }

    /// Reads the newest focus and reports it.
    unsafe fn focus_moved(&mut self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            let focused = match self.automation.GetFocusedElementBuildCache(&self.cache) {
                Ok(element) => {
                    let described = describe(&element);
                    let own = element
                        .CachedProcessId()
                        .is_ok_and(|process| u32::try_from(process) == Ok(std::process::id()));
                    if text_input::passes_over(&described, own) {
                        tracing::debug!(
                            control_type = described.control_type,
                            class = %described.class_name,
                            own,
                            "a focus event that leaves the focus where it was"
                        );
                        return;
                    }
                    Some((element, described))
                }
                // The app is busy or the element went away as it was read: the next focus
                // event says where the focus went, and a lower and raise now would flap.
                Err(error) if passing(error.code()) => {
                    tracing::debug!(%error, "the focus could not be read for now");
                    return;
                }
                // The focus may be nowhere, or in an app that UI Automation cannot reach (a
                // secure desktop, or an elevated app under a host that is not): no text field.
                Err(error) => {
                    if self.field.is_some() {
                        tracing::info!(%error, "the focus left the text field for one UI Automation cannot read");
                    }
                    None
                }
            };
            if self.field.is_some()
                && let Some((element, described)) = &focused
                && text_input::classify(described).is_none()
            {
                // Says what took the focus when the keyboard lowers without the user.
                tracing::info!(
                    control_type = described.control_type,
                    class = %described.class_name,
                    process = element.CachedProcessId().unwrap_or_default(),
                    "the focus left the text field"
                );
            }
            let field = focused.and_then(|(element, described)| {
                let field = text_input::classify(&described)?;
                Some((element, field))
            });
            let read = field
                .as_ref()
                .filter(|(_, field)| self.max_text > 0 && !field.sensitive)
                .map(|(element, _)| element.clone());
            self.follow(read);
            // Focus events also come for the element that has it; only another one counts.
            let same = match (&self.field, &field) {
                (Some((before, _)), Some((element, _))) => self
                    .automation
                    .CompareElements(before, element)
                    .is_ok_and(|same| same.as_bool()),
                _ => false,
            };
            if let Some((element, _)) = &field
                && !same
            {
                self.element = match runtime_id(element) {
                    Some(runtime_id) => text_input::element_id(&runtime_id),
                    None => {
                        // Still another element: the client hears the move, but cannot tell
                        // this field from one it closed the keyboard on before.
                        tracing::debug!("the focused field has no UI Automation runtime id");
                        self.element.wrapping_add(1).max(1)
                    }
                };
            }
            self.field = field;
            self.report(report);
        }
    }

    /// Reports the followed field again with its new text.
    unsafe fn text_changed(&mut self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            if self.followed.is_some() {
                self.report(report);
            }
        }
    }

    unsafe fn report(&self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            report(self.field.as_ref().map(|(element, field)| {
                Focused {
                    field: *field,
                    element: self.element,
                    surrounding: (self.max_text > 0 && !field.sensitive)
                        .then(|| read_text(element, self.max_text))
                        .flatten(),
                }
            }));
        }
    }

    /// Follows the text events of `element` only, or of none.
    unsafe fn follow(&mut self, element: Option<IUIAutomationElement>) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            let same = match (&self.followed, &element) {
                (Some(followed), Some(element)) => self
                    .automation
                    .CompareElements(followed, element)
                    .is_ok_and(|same| same.as_bool()),
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            if let Some(followed) = self.followed.take() {
                for event in TEXT_EVENTS {
                    let _ = self.automation.RemoveAutomationEventHandler(
                        event,
                        &followed,
                        &self.text_handler,
                    );
                }
            }
            if let Some(element) = element {
                for event in TEXT_EVENTS {
                    if let Err(error) = self.automation.AddAutomationEventHandler(
                        event,
                        &element,
                        TreeScope_Element,
                        None,
                        &self.text_handler,
                    ) {
                        // The field's text still goes out with each focus change.
                        tracing::debug!(%error, "the focused field does not report text changes");
                    }
                }
                self.followed = Some(element);
            }
        }
    }

    unsafe fn stop(&mut self) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            self.follow(None);
            let _ = self
                .automation
                .RemoveFocusChangedEventHandler(&self.focus_handler);
            let _ = self.automation.RemoveAllEventHandlers();
        }
    }
}

/// Whether UI Automation failed to read the focus only for now: the app took too long or
/// refused the call while busy, or the element went away as it was read.
fn passing(error: HRESULT) -> bool {
    const UIA_E_ELEMENTNOTAVAILABLE: HRESULT = HRESULT(0x8004_0201_u32 as i32);
    const UIA_E_TIMEOUT: HRESULT = HRESULT(0x8013_1505_u32 as i32);
    const RPC_E_CALL_REJECTED: HRESULT = HRESULT(0x8001_0001_u32 as i32);
    const RPC_E_SERVERCALL_RETRYLATER: HRESULT = HRESULT(0x8001_010A_u32 as i32);
    [
        UIA_E_ELEMENTNOTAVAILABLE,
        UIA_E_TIMEOUT,
        RPC_E_CALL_REJECTED,
        RPC_E_SERVERCALL_RETRYLATER,
    ]
    .contains(&error)
}

/// What the focus watcher needs to know about `element`, from its cached properties.
unsafe fn describe(element: &IUIAutomationElement) -> FocusedElement {
    let string = |value: windows::core::Result<BSTR>| {
        value.map(|value| value.to_string()).unwrap_or_default()
    };
    // SAFETY: the caller holds `element` on its COM thread, built with the watcher's cache.
    unsafe {
        let value = element
            .GetCachedPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok();
        FocusedElement {
            control_type: element
                .CachedControlType()
                .map(|kind| kind.0)
                .unwrap_or_default(),
            enabled: element
                .CachedIsEnabled()
                .is_ok_and(|enabled| enabled.as_bool()),
            password: element
                .CachedIsPassword()
                .is_ok_and(|password| password.as_bool()),
            read_only: value
                .and_then(|value| value.CachedIsReadOnly().ok())
                .map(|read_only| read_only.as_bool()),
            text_pattern: element
                .GetCachedPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                .is_ok(),
            aria_role: string(element.CachedAriaRole()),
            name: string(element.CachedName()),
            automation_id: string(element.CachedAutomationId()),
            class_name: string(element.CachedClassName()),
            localized_type: string(element.CachedLocalizedControlType()),
        }
    }
}

/// The UI Automation runtime id of `element`, cached with the focused element and read only
/// when the focus moves to another field, on the watcher's thread.
unsafe fn runtime_id(element: &IUIAutomationElement) -> Option<Vec<i32>> {
    // SAFETY: the caller holds `element` on its COM thread; the array UI Automation returns
    // is ours, read while locked and destroyed once.
    unsafe {
        let array = element.GetRuntimeId().ok()?;
        if array.is_null() {
            return None;
        }
        let ids = (|| {
            if (*array).cDims != 1 || (*array).cbElements as usize != size_of::<i32>() {
                return None;
            }
            let len = usize::try_from((*array).rgsabound[0].cElements).ok()?;
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).ok()?;
            let ids = std::slice::from_raw_parts(data.cast::<i32>(), len).to_vec();
            let _ = SafeArrayUnaccessData(array);
            Some(ids)
        })();
        let _ = SafeArrayDestroy(array);
        ids
    }
}

/// The text around the cursor of `element`: up to `max_text` characters either side of the
/// selection from its text pattern, else its whole value without a selection. Core cuts it
/// to `max_text` bytes.
unsafe fn read_text(element: &IUIAutomationElement, max_text: usize) -> Option<Surrounding> {
    let characters = i32::try_from(max_text).unwrap_or(i32::MAX);
    // SAFETY: the caller holds `element` on its COM thread.
    unsafe {
        if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            && let Some(surrounding) = around_selection(&pattern, characters)
        {
            return Some(surrounding);
        }
        let value = element
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()?;
        Some(Surrounding {
            text: value.CurrentValue().ok()?.to_string(),
            selection: None,
        })
    }
}

unsafe fn around_selection(
    pattern: &IUIAutomationTextPattern,
    characters: i32,
) -> Option<Surrounding> {
    // SAFETY: the caller holds `pattern` on its COM thread.
    unsafe {
        let ranges = pattern.GetSelection().ok()?;
        if ranges.Length().ok()? < 1 {
            return None;
        }
        let selection = ranges.GetElement(0).ok()?;
        let before = selection.Clone().ok()?;
        before
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        before
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -characters,
            )
            .ok()?;
        let after = selection.Clone().ok()?;
        after
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &selection,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        after
            .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, characters)
            .ok()?;
        let before = before.GetText(characters).ok()?.to_string();
        // A selection longer than `characters` is cut; the window core keeps then starts at
        // the selection and ends inside it, so the text after it never shows.
        let selected = selection.GetText(characters).ok()?.to_string();
        let after = after.GetText(characters).ok()?.to_string();
        let start = before.encode_utf16().count();
        let end = start + selected.encode_utf16().count();
        Some(Surrounding {
            text: before + &selected + &after,
            selection: Some((start, end)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The tests count threads and stuck watchers, which are process-wide.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn failed(error: anyhow::Error) {
        panic!("UI Automation did not start: {error:#}");
    }

    fn wait_until(done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done(), "timed out");
    }

    #[test]
    fn the_watcher_starts_reports_the_focus_and_stops() {
        let _serial = serial();
        let reports = Arc::new(Mutex::new(0));
        let counted = reports.clone();
        let watch = FocusWatch::start(64, move |_| *counted.lock().unwrap() += 1, failed)
            .expect("the watcher starts");
        // The focus at start is reported right after the thread is up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while *reports.lock().unwrap() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(*reports.lock().unwrap() >= 1);
        // A watcher that did not stop would hang the test.
        watch.stop_and_wait();
    }

    #[test]
    fn a_watcher_stopped_right_after_start_ends() {
        let _serial = serial();
        // Stopped before, while and after the thread gets its message queue.
        for _ in 0..20 {
            let watch = FocusWatch::start(0, |_| {}, failed).expect("the watcher starts");
            let started = std::time::Instant::now();
            watch.stop_and_wait();
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
        }
        assert_eq!(*LINGERING.lock().unwrap(), 0);
    }

    /// The client's toggles pause and resume one thread; a paused watcher reports nothing.
    #[test]
    fn toggling_the_watch_keeps_one_thread() {
        let _serial = serial();
        let reports = Arc::new(Mutex::new(0));
        let counted = reports.clone();
        let spawned = SPAWNED.load(Ordering::SeqCst);
        let watch = FocusWatch::start(0, move |_| *counted.lock().unwrap() += 1, failed)
            .expect("the watcher starts");
        wait_until(|| *reports.lock().unwrap() >= 1);
        let thread = watch.control.state().thread;
        for round in 0..20 {
            watch.set(Watch::Off);
            watch.set(Watch::On {
                max_text: round % 2 * 64,
            });
        }
        watch.set(Watch::Off);
        let paused = *reports.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            *reports.lock().unwrap(),
            paused,
            "a paused watcher reported"
        );
        watch.set(Watch::On { max_text: 64 });
        wait_until(|| *reports.lock().unwrap() > paused);
        assert_eq!(watch.control.state().thread, thread);
        assert_eq!(SPAWNED.load(Ordering::SeqCst), spawned + 1);
        watch.stop_and_wait();
    }

    /// The id the client gets is the same for one element read twice, and another for another.
    #[test]
    fn element_ids_come_from_the_runtime_id() {
        std::thread::spawn(|| {
            // SAFETY: COM is initialized on this thread and every object is dropped before
            // CoUninitialize.
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
                {
                    let automation: IUIAutomation =
                        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
                    let root = automation.GetRootElement().unwrap();
                    let again = automation.GetRootElement().unwrap();
                    let id = runtime_id(&root).expect("the desktop has a runtime id");
                    assert!(!id.is_empty());
                    assert_eq!(runtime_id(&again), Some(id.clone()));
                    let child = automation
                        .CreateTreeWalker(&automation.CreateTrueCondition().unwrap())
                        .unwrap()
                        .GetFirstChildElement(&root);
                    if let Ok(child) = child
                        && let Some(child) = runtime_id(&child)
                    {
                        assert_ne!(text_input::element_id(&child), text_input::element_id(&id));
                    }
                }
                CoUninitialize();
            }
        })
        .join()
        .unwrap();
    }

    /// A busy app or an element that went away keeps the last report; access denied and
    /// anything else is a focus UI Automation cannot read, no text field.
    #[test]
    fn only_passing_errors_keep_the_last_report() {
        for code in [0x8004_0201_u32, 0x8013_1505, 0x8001_0001, 0x8001_010A] {
            assert!(passing(HRESULT(code as i32)), "{code:#x}");
        }
        for code in [0x8007_0005_u32, 0x8000_4005, 0x8000_4002] {
            assert!(!passing(HRESULT(code as i32)), "{code:#x}");
        }
    }

    /// A watcher stuck in UI Automation holds up the next one, which fails instead of
    /// adding a thread.
    #[test]
    fn a_watcher_after_a_stuck_one_fails() {
        let _serial = serial();
        *LINGERING.lock().unwrap() += 1;
        let error = Arc::new(Mutex::new(None));
        let noted = error.clone();
        let watch = FocusWatch::start(
            0,
            |_| panic!("reported behind a stuck watcher"),
            move |error| *noted.lock().unwrap() = Some(format!("{error:#}")),
        )
        .expect("the thread starts");
        wait_until(|| error.lock().unwrap().is_some());
        *LINGERING.lock().unwrap() -= 1;
        assert!(
            error
                .lock()
                .unwrap()
                .as_deref()
                .unwrap()
                .contains("still stuck"),
            "{:?}",
            error.lock().unwrap()
        );
        watch.stop_and_wait();
        assert_eq!(*LINGERING.lock().unwrap(), 0);
    }
}
