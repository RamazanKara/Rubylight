use super::*;

/// A hidden or disabled Play is not there to press yet: its one press would be lost.
#[test]
fn only_a_shown_enabled_button_is_pressable() {
    use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
    // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            windows::core::w!("Rubylight button test"),
            WS_POPUP,
            0,
            0,
            64,
            64,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    assert!(!pressable(window), "hidden");
    // SAFETY: The window was created on this thread above.
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
    }
    assert!(pressable(window));
    // SAFETY: as above.
    unsafe {
        let _ = EnableWindow(window, false);
    }
    assert!(!pressable(window), "disabled");
    // SAFETY: as above.
    unsafe { DestroyWindow(window).unwrap() };
    assert!(!pressable(window), "closed");
}

/// Nucleus Co-op's Play is found by the name its program gives it, read
/// through UI Automation from a WinForms button of another process.
#[test]
fn a_winforms_buttons_name_is_read_from_another_process() {
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let script = "Add-Type -AssemblyName System.Windows.Forms; \
            $f = New-Object System.Windows.Forms.Form; \
            $f.StartPosition = 'Manual'; $f.Location = New-Object System.Drawing.Point(-30000, -30000); \
            $f.ShowInTaskbar = $false; \
            $b = New-Object System.Windows.Forms.Button; $b.Name = 'rubylightProbe'; $b.Text = 'Go'; \
            $f.Controls.Add($b); [System.Windows.Forms.Application]::Run($f)";
    let child = Kill(
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .spawn()
            .unwrap(),
    );
    let processes = BTreeSet::from([child.0.id()]);
    let automation = Automation::new().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let names = loop {
        let names: Vec<String> = top_level(Some(&processes))
            .unwrap()
            .iter()
            .flat_map(|window| children(hwnd(window.id)))
            .filter(|child| {
                class_of(*child).is_some_and(|class| {
                    class.starts_with(FORM_CLASS) && class.contains(".BUTTON.")
                })
            })
            .filter_map(|child| control_name(&automation, child))
            .collect();
        if !names.is_empty() || std::time::Instant::now() > deadline {
            break names;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(names, ["rubylightProbe"]);
}

#[test]
fn a_label_is_clicked_where_its_text_starts() {
    assert_eq!(click_point(120, 24), (4, 12));
    // A label narrower than eight pixels is clicked in its middle.
    assert_eq!(click_point(6, 10), (3, 5));
    assert_eq!(click_point(0, 0), (0, 0));
}

/// #59: a process's Mandatory ASLR policy is read, and one that can't be
/// opened is an error, never "not forced".
#[test]
fn a_processes_mandatory_aslr_is_read_or_an_error() {
    let ours = forced_aslr(std::process::id());
    assert!(ours.is_ok(), "{ours:?}");
    assert!(forced_aslr(0).is_err());
}

/// Nucleus Co-op is raised above the window over its profile list for the
/// click, a topmost one too, and goes back below it after (#56).
#[test]
fn a_covered_window_is_raised_for_the_click_and_put_back() {
    struct Made(Vec<HWND>);
    impl Drop for Made {
        fn drop(&mut self) {
            for &window in &self.0 {
                // SAFETY: a window this thread made.
                let _ = unsafe { DestroyWindow(window) };
            }
        }
    }
    let make = |ex: WINDOW_EX_STYLE, owner: Option<HWND>| {
        // SAFETY: the class and text are NUL-terminated strings that outlive the call; the
        // owner, when given, is a window this thread made.
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | ex,
                &windows::core::HSTRING::from("Static"),
                &windows::core::HSTRING::from("Rubylight raise test"),
                WS_POPUP | WS_VISIBLE,
                -32000,
                -32000,
                300,
                200,
                owner,
                None,
                None,
                None,
            )
        }
        .unwrap()
    };
    // Whether `upper` is anywhere above `lower`.
    let above = |upper: HWND, lower: HWND| {
        let mut window = lower;
        // SAFETY: plain queries of window handles.
        while let Ok(prev) = unsafe { GetWindow(window, GW_HWNDPREV) } {
            if prev.is_invalid() {
                break;
            }
            if prev == upper {
                return true;
            }
            window = prev;
        }
        false
    };
    for over in [WINDOW_EX_STYLE::default(), WS_EX_TOPMOST] {
        let nucleus = make(Default::default(), None);
        // A box of its own over it stays where it is.
        let shown = make(Default::default(), Some(nucleus));
        let game = make(over, None);
        let _made = Made(vec![game, shown, nucleus]);
        // SAFETY: windows this thread made.
        unsafe {
            assert!(above(shown, nucleus) && above(game, shown));
            assert!(!topmost(nucleus));
            let raised = Raised::new(nucleus).unwrap();
            assert!(topmost(nucleus), "{over:?}");
            assert!(above(nucleus, game), "{over:?}");
            assert!(!topmost(shown) && above(game, shown), "{over:?}");
            drop(raised);
            assert!(!topmost(nucleus) && !topmost(shown), "{over:?}");
            assert_eq!(GetWindow(nucleus, GW_HWNDPREV).unwrap(), shown);
            assert!(above(game, shown), "{over:?}");
            assert_eq!(topmost(game), over == WS_EX_TOPMOST);
        }
    }
}

/// #56: a game window's title bar and sizing border are listed and
/// taken away; its other styles stay.
#[test]
fn a_windows_frame_is_listed_and_taken_away() {
    assert!(framed(WS_OVERLAPPEDWINDOW.0));
    assert!(framed(WS_POPUP.0 | WS_THICKFRAME.0));
    assert!(!framed(WS_POPUP.0 | WS_BORDER.0));
    assert_eq!(
        unframed((WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN).0),
        (WS_VISIBLE | WS_CLIPCHILDREN).0
    );
    // SAFETY: the class and text are NUL-terminated strings that outlive the call.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            &windows::core::HSTRING::from("Static"),
            &windows::core::HSTRING::from("Rubylight frame test"),
            WS_OVERLAPPEDWINDOW,
            -32000,
            -32000,
            300,
            200,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let listed = || {
        top_level(Some(&BTreeSet::from([std::process::id()])))
            .unwrap()
            .into_iter()
            .find(|w| w.id == window.0 as usize as u64)
            .unwrap()
    };
    assert!(listed().framed);
    let mut desktop = Win32::default();
    let id = window.0 as usize as u64;
    // This thread owns the window and runs no message loop while the
    // pass waits, as a hung game: the pass gives up in time instead of
    // waiting without end, and starts no second change meanwhile.
    let before = std::time::Instant::now();
    let error = desktop.unframe(id).unwrap_err().to_string();
    assert!(
        error.contains("did not take its frame's removal"),
        "{error}"
    );
    assert!(before.elapsed() < Duration::from_secs(2));
    let error = desktop.unframe(id).unwrap_err().to_string();
    assert!(error.contains("has not yet taken"), "{error}");
    // Once its messages run, the window takes the change.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while desktop.unframing.load(Ordering::Acquire) || listed().framed {
        assert!(std::time::Instant::now() < deadline, "the frame stayed");
        let mut message = MSG::default();
        // SAFETY: `message` is a live local; only this thread's messages are run.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Once is enough; again changes nothing, and waits on no thread.
    desktop.unframe(id).unwrap();
    // SAFETY: a window this thread made.
    unsafe { DestroyWindow(window).unwrap() };
    assert!(desktop.unframe(window.0 as usize as u64).is_err());
}

#[test]
fn the_desktop_lists_this_pc_s_windows_and_monitors() {
    let mut desktop = Win32::default();
    let processes = desktop.processes().unwrap();
    assert!(processes.iter().any(|p| p.pid == std::process::id()));
    let monitors = desktop.monitors().unwrap();
    let screen = desktop.virtual_screen();
    // A desktop without a monitor (a service test run) has an empty screen.
    for monitor in &monitors {
        assert!(
            i64::from(monitor.x) >= i64::from(screen.x) && monitor.right() <= screen.right(),
            "{monitor:?} outside {screen:?}"
        );
    }
    desktop.windows().unwrap();
    // No process of this test has a window labelled like the profile.
    assert_eq!(
        desktop
            .label(&BTreeSet::from([std::process::id()]), "Rubylight")
            .unwrap(),
        None
    );
    // Nor a message box; one Windows shows for another program may be
    // up.
    let own = BTreeSet::from([std::process::id()]);
    let before = desktop.message_boxes(&own).unwrap();
    let new = |desktop: &mut Win32| -> Vec<MessageBox> {
        let mut boxes = desktop.message_boxes(&own).unwrap();
        boxes.retain(|b| !before.contains(b));
        boxes
    };

    // A box as MessageBox makes it, off every screen and never active:
    // a dialog with its icon and its text, a few hundred characters as
    // OneDrive's warning in Nucleus Co-op. A plain window beside it is
    // no message box. In this test, not a test of its own: the empty
    // list above would see them.
    struct Made(Vec<HWND>);
    impl Drop for Made {
        fn drop(&mut self) {
            for &window in self.0.iter().rev() {
                // SAFETY: a window this thread made; one its parent took along fails harmlessly.
                let _ = unsafe { DestroyWindow(window) };
            }
        }
    }
    let make = |class: &str, text: &str, ex: WINDOW_EX_STYLE, style, parent| {
        // SAFETY: the class and text are NUL-terminated strings that outlive the call; the
        // parent, when given, is a window this thread made.
        unsafe {
            CreateWindowExW(
                ex,
                &windows::core::HSTRING::from(class),
                &windows::core::HSTRING::from(text),
                style,
                -32000,
                -32000,
                300,
                200,
                parent,
                None,
                None,
                None,
            )
        }
        .unwrap()
    };
    let hidden = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let said = "Your Documents folder is in OneDrive. ".repeat(11);
    assert!(said.len() > 400);
    let mut made = Made(Vec::new());
    let dialog = make(
        DIALOG_CLASS,
        "Rubylight test box",
        hidden,
        WS_POPUP | WS_VISIBLE,
        None,
    );
    made.0.push(dialog);
    let child = WS_CHILD | WS_VISIBLE;
    made.0
        .push(make("Static", "", Default::default(), child, Some(dialog)));
    made.0.push(make(
        "Static",
        &said,
        Default::default(),
        child,
        Some(dialog),
    ));
    made.0.push(make(
        "Static",
        "not a box",
        hidden,
        WS_POPUP | WS_VISIBLE,
        None,
    ));
    assert_eq!(
        new(&mut desktop),
        [MessageBox {
            window: dialog.0 as usize as u64,
            caption: "Rubylight test box".into(),
            text: said.clone(),
        }]
    );
    // With no process, only the boxes Windows shows: not this one.
    assert!(
        desktop
            .message_boxes(&BTreeSet::new())
            .unwrap()
            .iter()
            .all(|b| b.window != dialog.0 as usize as u64)
    );
    drop(made);
    assert_eq!(new(&mut desktop), []);

    // A form shown with ShowDialog: the enabled form owned by the window
    // it disabled, which is closed with what it says. ShowDialog
    // disables the thread's other windows too: a form open beside it
    // and the modal form below a nested one are left alone, as are a
    // native window over the disabled one (a tooltip, a message box)
    // and a form whose owner is enabled.
    unsafe extern "system" fn plain(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: the message as Windows passed it.
        unsafe { DefWindowProcW(window, message, w, l) }
    }
    let class = "WindowsForms10.Window.8.app.rubylight-test";
    let name = windows::core::HSTRING::from(class);
    let registered = WNDCLASSW {
        lpfnWndProc: Some(plain),
        lpszClassName: windows::core::PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: the class name outlives the call; a second registration fails harmlessly.
    unsafe { RegisterClassW(&registered) };
    let disable = |window: HWND| {
        // SAFETY: a window this thread made.
        let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(window, false) };
    };
    let mut made = Made(Vec::new());
    let popup = WS_POPUP | WS_VISIBLE;
    let owner = make(class, "Nucleus Co-op", hidden, popup, None);
    made.0.push(owner);
    let form = make(class, "", hidden, popup, Some(owner));
    made.0.push(form);
    made.0.push(make(
        "Static",
        "The profile can't be loaded.",
        Default::default(),
        WS_CHILD | WS_VISIBLE,
        Some(form),
    ));
    let beside = make(class, "beside", hidden, popup, Some(owner));
    made.0.push(beside);
    made.0
        .push(make("Static", "tooltip", hidden, popup, Some(owner)));
    made.0
        .push(make(DIALOG_CLASS, "box", hidden, popup, Some(owner)));
    let free = make(class, "free", hidden, popup, None);
    made.0.push(free);
    made.0.push(make(class, "owned", hidden, popup, Some(free)));
    assert!(desktop.close_forms(&own).unwrap().is_empty());
    disable(owner);
    disable(beside);
    assert_eq!(
        desktop.close_forms(&own).unwrap(),
        [MessageBox {
            window: form.0 as usize as u64,
            caption: String::new(),
            text: "The profile can't be loaded.".into(),
        }]
    );
    // A form shown over that one: only it is.
    let nested = make(class, "nested", hidden, popup, Some(form));
    made.0.push(nested);
    disable(form);
    let closed: Vec<u64> = desktop
        .close_forms(&own)
        .unwrap()
        .iter()
        .map(|form| form.window)
        .collect();
    assert_eq!(closed, [nested.0 as usize as u64]);
}
