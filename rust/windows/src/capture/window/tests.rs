use super::*;

#[test]
fn a_window_that_will_send_no_frame_ends_the_wait_for_its_first() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SW_SHOWNOACTIVATE, ShowWindow, WINDOW_EX_STYLE, WS_POPUP,
    };
    // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            windows::core::w!("Rubylight capture test"),
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
    }?;
    let handle = window.0 as usize;
    let error = check_awaited(handle).unwrap_err().to_string();
    assert!(error.starts_with("the game window is hidden"), "{error}");
    // The usual check still passes it, as the capture's other uses need.
    check(handle)?;
    // SAFETY: The window was created on this thread above.
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
    }
    check_awaited(handle)?;
    // SAFETY: The window was created on this thread above.
    unsafe { DestroyWindow(window)? };
    let error = check_awaited(handle).unwrap_err().to_string();
    assert!(error.starts_with("the game window closed"), "{error}");
    Ok(())
}

#[test]
fn window_targets_parse_and_print_back() {
    let target = WindowTarget::parse(r"window:132456@\\.\DISPLAY2")
        .unwrap()
        .unwrap();
    assert_eq!(target, WindowTarget::new(132456, r"\\.\DISPLAY2"));
    assert_eq!(target.to_string(), r"window:132456@\\.\DISPLAY2");
    assert_eq!(target.handle(), 132456);
    assert_eq!(
        WindowTarget::parse(r"window:0x2056c@\\.\DISPLAY1")
            .unwrap()
            .unwrap()
            .window,
        0x2056c
    );
    let round = WindowTarget::new(77, r"\\.\DISPLAY9");
    assert_eq!(
        WindowTarget::parse(&round.to_string()).unwrap(),
        Some(round.clone())
    );
    // With the owning process.
    let owned = round.owned_by(4242);
    assert_eq!(owned.to_string(), r"window:77/4242@\\.\DISPLAY9");
    assert_eq!(
        WindowTarget::parse(r"window:0x4d/4242@\\.\DISPLAY9").unwrap(),
        Some(owned)
    );
}

#[test]
fn display_names_are_not_window_targets() {
    for name in [r"\\.\DISPLAY1", "", "windows", r"\\.\DISPLAY1@window:5"] {
        assert_eq!(WindowTarget::parse(name).unwrap(), None, "{name}");
    }
}

#[test]
fn malformed_window_targets_are_refused_not_opened_as_displays() {
    for (name, says) in [
        ("window:", "is not window:<hwnd>@<display>"),
        ("window:123", "is not window:<hwnd>@<display>"),
        (r"window:@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
        (r"window:abc@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
        (r"window:-5@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
        (r"window:0x@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
        (r"window:0@\\.\DISPLAY1", "names no window"),
        ("window:123@", "names no display"),
        (
            r"window:123/@\\.\DISPLAY1",
            "is not window:<hwnd>@<display>",
        ),
        (
            r"window:123/x@\\.\DISPLAY1",
            "is not window:<hwnd>@<display>",
        ),
        (r"window:/5@\\.\DISPLAY1", "is not window:<hwnd>@<display>"),
        (r"window:123/0@\\.\DISPLAY1", "names no process"),
    ] {
        let error = WindowTarget::parse(name).unwrap_err().to_string();
        assert!(error.contains(says), "{name}: {error}");
    }
}

#[test]
fn window_targets_refuse_backends_that_capture_whole_displays() {
    for kind in ["ddx", "dxgi", "idd-push"] {
        let error = refuse_backend(kind).unwrap_err().to_string();
        assert!(
            error.contains("only be captured with Windows Graphics Capture"),
            "{error}"
        );
    }
    for kind in ["wgc", "auto", ""] {
        refuse_backend(kind).unwrap();
    }
}

#[test]
fn builds_without_window_capture_are_refused_without_another_backend() {
    for build in [16299, 17134, 17763] {
        let error = require_capture(Some(build)).unwrap_err().to_string();
        assert!(
            error.contains("WGC window capture needs Windows build 18362"),
            "{error}"
        );
        assert!(!error.contains("Desktop Duplication"), "{error}");
    }
    for build in [Some(18362), Some(19045), Some(26200), None] {
        require_capture(build).unwrap();
    }
}

#[test]
fn a_closed_or_minimised_window_is_refused_loudly() {
    assert_eq!(unavailable(true, false), None);
    assert_eq!(unavailable(false, false), Some("the game window closed"));
    // IsIconic of a destroyed handle is false anyway; closed wins.
    assert_eq!(unavailable(false, true), Some("the game window closed"));
    assert!(unavailable(true, true).unwrap().contains("minimised"));
}

#[test]
fn windows_are_checked_until_they_close() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_MINIMIZE, WS_POPUP,
    };
    // Hidden windows of the predefined STATIC class: nothing appears.
    let open = |style| {
        // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                windows::core::w!("Rubylight capture test"),
                style,
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
    };
    let normal = open(WS_POPUP)?;
    let minimised = open(WS_POPUP | WS_MINIMIZE)?;
    let result = (|| -> Result<()> {
        check(normal.0 as usize)?;
        let error = check(minimised.0 as usize).unwrap_err().to_string();
        assert!(error.starts_with("the game window is minimised"), "{error}");
        Ok(())
    })();
    // SAFETY: Both windows were created on this thread above.
    unsafe {
        DestroyWindow(normal)?;
        DestroyWindow(minimised)?;
    }
    result?;
    let error = check(normal.0 as usize).unwrap_err().to_string();
    assert_eq!(
        error,
        format!("the game window closed (window {:#x})", normal.0 as usize)
    );
    Ok(())
}

#[test]
fn a_window_another_process_owns_is_refused() {
    owned_by(5, 40, 40).unwrap();
    assert_eq!(
        owned_by(5, 41, 40).unwrap_err().to_string(),
        "the window 0x5 belongs to process 41, not 40; the window it named has closed"
    );
}

#[test]
fn a_window_on_another_display_than_named_is_refused() -> Result<()> {
    assert!(on_display(5, r"\\.\DISPLAY1", r"\\.\display1").is_ok());
    let error = on_display(5, r"\\.\DISPLAY1", r"\\.\DISPLAY2")
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        r"the window 0x5 is on \\.\DISPLAY1, not on \\.\DISPLAY2; name the display it is on"
    );
    // A real (hidden) window: its own display passes, a name no display
    // has is refused, never captured on the display it is on.
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
    };
    // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            windows::core::w!("Rubylight capture test"),
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
    }?;
    let handle = window.0 as usize;
    let result = (|| -> Result<()> {
        let shown_on = display_of(handle)?;
        assert!(shown_on.starts_with(r"\\.\"), "{shown_on}");
        crate::capture::check_window(&WindowTarget::new(handle as u64, &shown_on))?;
        let error =
            crate::capture::check_window(&WindowTarget::new(handle as u64, r"\\.\DISPLAY99"))
                .unwrap_err()
                .to_string();
        assert!(error.contains(r"not on \\.\DISPLAY99"), "{error}");
        // Its own process passes; another one's is a reused handle.
        let own = WindowTarget::new(handle as u64, &shown_on).owned_by(std::process::id());
        crate::capture::check_window(&own)?;
        let error = crate::capture::check_window(&own.owned_by(std::process::id() + 1))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("belongs to process {}", std::process::id())),
            "{error}"
        );
        Ok(())
    })();
    // SAFETY: The window was created on this thread above.
    unsafe { DestroyWindow(window)? };
    result
}

#[test]
fn a_resized_window_recreates_the_pool_and_skips_stale_frames() {
    // Steady: the window, the pool and the frame agree.
    assert_eq!(frame((1280, 720), (1280, 720), (1280, 720)), Frame::Use);
    // The window grew: the frame still has the old pool's texture.
    assert_eq!(
        frame((1280, 720), (1920, 1080), (1280, 720)),
        Frame::Resize((1920, 1080))
    );
    // And shrank.
    assert_eq!(
        frame((1920, 1080), (800, 600), (1920, 1080)),
        Frame::Resize((800, 600))
    );
    // After the pool was recreated: a frame queued before it has the
    // window's new size but the old texture, and is skipped.
    assert_eq!(frame((1920, 1080), (1920, 1080), (1280, 720)), Frame::Skip);
    assert_eq!(frame((1920, 1080), (1920, 1080), (1920, 1080)), Frame::Use);
    // An empty window never reaches the pool.
    assert_eq!(frame((1280, 720), (0, 0), (1280, 720)), Frame::Skip);
    assert_eq!(frame((1280, 720), (1280, 0), (1280, 720)), Frame::Skip);
}
