//! The focused text field for keyboard follows focus (0x553B, Experimental): a thread that
//! owns UI Automation's focus-changed handler and reports each text field the focus moves
//! to, or that it left them. With `max_text` above 0 it also follows the field's text and
//! selection and reads up to that much around the cursor, never in a password field.
//!
//! UI Automation calls the handlers on its own threads. They only wake the watcher, which
//! reads the newest focus itself, so a burst of changes costs one read and nothing here
//! runs on the video or input paths.

use anyhow::{Context, Result};
use butterpollo_core::text_input::{self, Field, Focused, FocusedElement, Surrounding};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use windows::{
    Win32::{
        Foundation::{LPARAM, WPARAM},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
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
                UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
                UIA_TextPatternId, UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId,
            },
            WindowsAndMessaging::{
                DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
                PostThreadMessageW, WM_APP, WM_QUIT,
            },
        },
    },
    core::{BSTR, Ref, implement},
};

/// The focus moved.
const WAKE_FOCUS: u32 = WM_APP + 1;
/// The followed field's text or selection changed.
const WAKE_TEXT: u32 = WM_APP + 2;
/// The text events followed in a field whose text is read.
const TEXT_EVENTS: [UIA_EVENT_ID; 2] = [
    UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
];

/// Watches the focus until dropped. Neither starting nor dropping waits for the watcher's
/// thread: the control loop starts and stops it, and a UI Automation call stuck in a hung
/// app must not hold up input.
pub struct FocusWatch {
    control: Arc<Control>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// How the watcher is told to stop: its thread id once it has a message queue, and a flag
/// for a stop that comes before.
#[derive(Default)]
struct Control {
    thread: AtomicU32,
    stop: AtomicBool,
}

impl FocusWatch {
    /// Calls `report` on the watcher's thread with the focused text field, or `None` once the
    /// focus left text fields, first for the focus at start. `max_text` above 0 also reads up
    /// to that many bytes around the cursor of a field that is not a password, and reports
    /// again when its text or selection changes. `failed` gets why UI Automation did not
    /// start, on the same thread; `Err` only when there is no thread.
    pub fn start(
        max_text: usize,
        report: impl Fn(Option<Focused>) + Send + 'static,
        failed: impl FnOnce(anyhow::Error) + Send + 'static,
    ) -> Result<Self> {
        let control = Arc::new(Control::default());
        let shared = control.clone();
        let worker = std::thread::Builder::new()
            .name("text-focus".into())
            .spawn(move || {
                if let Err(error) = run(max_text, &report, &shared) {
                    failed(error);
                }
            })
            .context("the focus watcher thread did not start")?;
        Ok(Self {
            control,
            worker: Some(worker),
        })
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
        // The watcher stores its id before it reads the flag, and this sets the flag before it
        // reads the id: one of the two sees the other.
        self.control.stop.store(true, Ordering::SeqCst);
        let thread = self.control.thread.load(Ordering::SeqCst);
        if thread != 0 {
            post(thread, WM_QUIT);
        }
        // Detached: it ends once UI Automation returns.
        drop(self.worker.take());
    }
}

fn post(thread: u32, message: u32) {
    // SAFETY: PostThreadMessageW only takes a thread id and plain values; a stale id fails.
    unsafe {
        let _ = PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0));
    }
}

fn run(max_text: usize, report: &dyn Fn(Option<Focused>), control: &Control) -> Result<()> {
    // SAFETY: COM is initialized and uninitialized on this thread, every UI Automation object
    // is created, used and released on it before CoUninitialize, and `message` is a live local
    // for every message call.
    unsafe {
        let mut message = MSG::default();
        // The thread needs a message queue before anything posts to it.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        control.thread.store(GetCurrentThreadId(), Ordering::SeqCst);
        if control.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("COM did not start for the focus watcher")?;
        // A watcher that was stopped while it read the focus must not report after the one
        // that replaced it.
        let report = |focused: Option<Focused>| {
            if !control.stop.load(Ordering::SeqCst) {
                report(focused);
            }
        };
        let report: &dyn Fn(Option<Focused>) = &report;
        let result = Watcher::new(max_text, GetCurrentThreadId()).map(|mut watcher| {
            watcher.focus_moved(report);
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let (mut focus, mut text) = (false, false);
                loop {
                    match message.message {
                        WAKE_FOCUS => focus = true,
                        WAKE_TEXT => text = true,
                        // COM's own window in this apartment.
                        _ => {
                            DispatchMessageW(&message);
                        }
                    }
                    if !PeekMessageW(&mut message, None, WAKE_FOCUS, WAKE_TEXT, PM_REMOVE).as_bool()
                    {
                        break;
                    }
                }
                if focus {
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
    /// Counts on each time the focus moves to another text field ([`Focused::element`]).
    element: u32,
    /// The field whose text events are followed.
    followed: Option<IUIAutomationElement>,
}

impl Watcher {
    unsafe fn new(max_text: usize, thread: u32) -> Result<Self> {
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
            ] {
                cache.AddProperty(property)?;
            }
            cache.AddPattern(UIA_ValuePatternId)?;
            cache.AddPattern(UIA_TextPatternId)?;
            let wake = Wake(thread);
            let focus_handler: IUIAutomationFocusChangedEventHandler = wake.into();
            let text_handler: IUIAutomationEventHandler =
                windows::core::Interface::cast(&focus_handler)?;
            automation
                .AddFocusChangedEventHandler(&cache, &focus_handler)
                .context("UI Automation does not report focus changes")?;
            Ok(Self {
                automation,
                cache,
                focus_handler,
                text_handler,
                max_text,
                field: None,
                element: 0,
                followed: None,
            })
        }
    }

    /// Reads the newest focus and reports it.
    unsafe fn focus_moved(&mut self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            // The focus may be nowhere, or in an app that UI Automation cannot reach (a
            // secure desktop, or an elevated app under a host that is not): no text field.
            let field = self
                .automation
                .GetFocusedElementBuildCache(&self.cache)
                .ok()
                .and_then(|element| {
                    let field = text_input::classify(&describe(&element))?;
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
            if field.is_some() && !same {
                self.element = self.element.wrapping_add(1);
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

    fn failed(error: anyhow::Error) {
        panic!("UI Automation did not start: {error:#}");
    }

    #[test]
    fn the_watcher_starts_reports_the_focus_and_stops() {
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
        // Stopped before, while and after the thread gets its message queue.
        for _ in 0..20 {
            let watch = FocusWatch::start(0, |_| {}, failed).expect("the watcher starts");
            let started = std::time::Instant::now();
            watch.stop_and_wait();
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
        }
    }
}
