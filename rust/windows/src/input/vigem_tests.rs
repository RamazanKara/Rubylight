use super::*;
use crate::input::{Event, gamepads::Gamepads};
use butterpollo_core::input_policy::VIGEM_X360;
use std::time::{Duration, Instant};

#[test]
fn missing_usb_read_does_not_disconnect_a_ready_target() {
    let failure =
        |code: WIN32_ERROR| Err(windows::core::Error::from_hresult(code.to_hresult()).into());
    assert!(report_result(Ok(())).is_ok());
    assert!(report_result(failure(ERROR_NO_MORE_ITEMS)).is_ok());
    assert!(report_result(failure(ERROR_ACCESS_DENIED)).is_err());
    assert!(report_result(failure(ERROR_INVALID_PARAMETER)).is_err());
}

#[test]
fn failed_unplug_retains_the_targets_feedback() -> Result<()> {
    let bus = INVALID_HANDLE_VALUE;
    let mut target = Target {
        bus,
        serial: 1,
        notification: Some(Notification {
            bus,
            event: event()?,
            pending: Box::new(Pending {
                overlapped: OVERLAPPED::default(),
                data: [0; 16],
            }),
            running: false,
            kind: Kind::X360,
            lightbar: None,
        }),
        ds4: None,
    };
    assert!(target.unplug().is_err());
    assert_eq!(target.serial, 1);
    assert!(target.notification.is_some());
    // Not to unplug it again for real on the invalid handle.
    target.serial = 0;
    Ok(())
}

#[test]
fn wire_headers_match_bus_shared_h() {
    assert_eq!(PLUGIN, 0x2aa004);
    assert_eq!(WAIT_READY, 0x2aa010);
    assert_eq!(XUSB_SUBMIT, 0x2aa808);
    assert_eq!(XUSB_NOTIFICATION, 0x2ae804);
    assert_eq!(DS4_SUBMIT, 0x2aa80c);
    assert_eq!(DS4_NOTIFICATION, 0x2aa810);
    assert_eq!(Kind::X360.notification().1, 12);
    assert_eq!(Kind::Ds4.notification().1, 16);
    assert_eq!(packet(8, 0x12345678), [8, 0, 0, 0, 0x78, 0x56, 0x34, 0x12]);
}

#[test]
fn xusb_buttons_axes_and_triggers_match_vibepollo() {
    assert_eq!(xusb_report(0, 0, 0, &[0; 4]), [0; 12]);
    for bit in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 15] {
        let report = xusb_report(1 << bit, 0, 0, &[0; 4]);
        assert_eq!(u16::from_le_bytes([report[0], report[1]]), 1 << bit);
    }
    assert_eq!(xusb_report(0x200000, 0, 0, &[0; 4])[..2], [0, 4]);
    assert_eq!(xusb_report(0xffff0800, 0, 0, &[0; 4])[..2], [0, 4]);
    assert_eq!(
        xusb_report(0x9231, 17, 255, &[i16::MIN, i16::MAX, -1, 0x1234]),
        [0x31, 0x92, 17, 255, 0, 128, 255, 127, 255, 255, 0x34, 0x12]
    );
}

#[test]
fn a_report_built_on_the_stack_matches_the_xusb_submit_packet() {
    let sticks = [i16::MIN, i16::MAX, -1, 0x1234];
    let mut expected = packet(20, 0x0102_0304);
    expected[8..].copy_from_slice(&xusb_report(0x9231, 17, 255, &sticks));
    assert_eq!(
        submit_packet(0x0102_0304, 0x9231, 17, 255, &sticks).to_vec(),
        expected
    );
}

#[test]
fn a_notification_that_will_not_re_arm_keeps_the_report_that_arrived() -> Result<()> {
    // A request that completed with a rumble, on a bus no new request starts on.
    let bus = INVALID_HANDLE_VALUE;
    let mut data = [0; 16];
    data[..12].copy_from_slice(&[12, 0, 0, 0, 1, 0, 0, 0, 255, 128, 0, 0]);
    let mut notification = Notification {
        bus,
        event: event()?,
        pending: Box::new(Pending {
            overlapped: OVERLAPPED {
                Internal: 0,
                InternalHigh: 12,
                ..Default::default()
            },
            data,
        }),
        running: true,
        kind: Kind::X360,
        lightbar: None,
    };
    assert!(notification.completed());
    assert_eq!(
        notification.poll()?,
        Some((4, vec![255, 255, 128, 128, 0, 0, 0, 0]))
    );
    assert!(!notification.running);
    // The next poll starts it again and reports why it will not start.
    assert!(notification.poll().is_err());
    Ok(())
}

#[test]
fn notifications_use_a_vhf_xbox_pads_rumble_format() {
    assert_eq!(
        feedback_report(
            Kind::X360,
            &[12, 0, 0, 0, 7, 0, 0, 0, 255, 128, 3, 0],
            &mut None
        ),
        (4, vec![255, 255, 128, 128, 0, 0, 0, 0])
    );
}

/// #108: a DualShock 4's rumble and lightbar arrive as the VHF DualShock 4's
/// do, so the control stream sends Moonlight's RGB LED message; a colour
/// already sent is not sent again, as in Vibepollo.
#[test]
fn ds4_notifications_carry_the_lightbar_once_per_colour() {
    let mut lightbar = None;
    let ds4 = |small, large, colour: [u8; 3]| {
        let mut data = [16, 0, 0, 0, 7, 0, 0, 0, small, large, 0, 0, 0, 0, 0, 0];
        data[10..13].copy_from_slice(&colour);
        data
    };
    assert_eq!(
        feedback_report(Kind::Ds4, &ds4(128, 255, [12, 34, 56]), &mut lightbar),
        (5, vec![255, 255, 128, 128, 12, 34, 56, 1])
    );
    // A rumble change repeats the colour: only the rumble goes.
    assert_eq!(
        feedback_report(Kind::Ds4, &ds4(0, 0, [12, 34, 56]), &mut lightbar),
        (5, vec![0, 0, 0, 0, 12, 34, 56, 0])
    );
    assert_eq!(
        feedback_report(Kind::Ds4, &ds4(0, 0, [0, 0, 255]), &mut lightbar),
        (5, vec![0, 0, 0, 0, 0, 0, 255, 1])
    );
    // The host turns those into the packets moonlight-common-c reads.
    assert_eq!(lightbar, Some([0, 0, 255]));
}

#[test]
fn ds4_dpad_buttons_and_axes_match_vibepollo() {
    let mut report = Ds4Report::new();
    assert_eq!(&report.0[..9], &[128, 128, 128, 128, 8, 0, 0, 0, 0]);
    for (flags, hat) in [
        (0, 8),
        (1, 0),
        (9, 1),
        (8, 2),
        (10, 3),
        (2, 4),
        (6, 5),
        (4, 6),
        (5, 7),
        (15, 1),
    ] {
        report.state(flags, 0, 0, &[0; 4]);
        assert_eq!(report.0[4] & 15, hat);
    }
    for (flags, buttons) in [
        (0x40, 0x4000u16),
        (0x80, 0x8000),
        (0x100, 0x100),
        (0x200, 0x200),
        (0x10, 0x2000),
        (0x20, 0x1000),
        (0x1000, 0x20),
        (0x2000, 0x40),
        (0x4000, 0x10),
        (0x8000, 0x80),
    ] {
        report.state(flags, 0, 0, &[0; 4]);
        assert_eq!(u16::from_le_bytes([report.0[4], report.0[5]]), buttons | 8);
    }
    // Guide is PS; the touchpad button and Share both click the touchpad.
    report.state(0x300400, 1, 254, &[i16::MIN, i16::MAX, i16::MAX, i16::MIN]);
    assert_eq!(&report.0[..9], &[0, 2, 255, 255, 8, 12, 3, 1, 254]);
    report.state(0x100000, 0, 0, &[0; 4]);
    assert_eq!(report.0[6], 2);
    report.state(0, 0, 0, &[0; 4]);
    assert_eq!(&report.0[..9], &[127, 129, 127, 129, 8, 0, 0, 0, 0]);
}

#[test]
fn ds4_touch_packs_two_contacts_and_retains_tracking_until_release() {
    let mut report = Ds4Report::new();
    report.touch(0, 1, [0.5, 1.]);
    assert_eq!(
        &report.0[32..42],
        &[1, 1, 1, 0xc0, 0xf3, 0x3a, 0x80, 0, 0, 0]
    );
    report.touch(1, 1, [1., 0.]);
    assert_eq!(&report.0[38..42], &[1, 0x80, 7, 0]);
    report.touch(0, 3, [0., 0.]);
    assert_eq!(&report.0[34..38], &[1, 0, 0, 0]);
    report.touch(0, 2, [0., 0.]);
    assert_eq!(report.0[34], 0x81);
    report.touch(0, 1, [0., 0.]);
    assert_eq!(report.0[34], 2);
    report.touch(0, 5, [0., 0.]);
    assert_eq!(report.0[34], 0x82);
    assert_eq!(report.0[38], 0x81);
    assert_eq!(report.0[33], 6);
    // The 7-bit tracking number wraps without ever reading as lifted.
    for _ in 0..130 {
        report.touch(0, 1, [0., 0.]);
        assert_eq!(report.0[34] & 0x80, 0);
        report.touch(0, 4, [0., 0.]);
    }
    // Hover changes nothing.
    let before = report.0;
    report.touch(0, 0, [0.5, 0.5]);
    assert_eq!(report.0, before);
}

#[test]
fn ds4_motion_uses_vigem_calibration_and_preserves_other_fields() {
    let mut report = Ds4Report::new();
    report.touch(0, 1, [0.25, 0.5]);
    report.motion(1, &[9.80665, -9.80665, 0.]);
    report.motion(2, &[90., -180., 0.]);
    let axes = |report: &Ds4Report, offset: usize| -> [i16; 3] {
        std::array::from_fn(|axis| {
            let at = offset + axis * 2;
            i16::from_le_bytes([report.0[at], report.0[at + 1]])
        })
    };
    // Golden values from Vibepollo's MPS2_TO_DS4_ACCEL / DPS_TO_DS4_GYRO
    // and APPLY_CALIBRATION, with float (not double) arithmetic.
    assert_eq!(axes(&report, 18), [7810, -8115, -499]);
    assert_eq!(axes(&report, 12), [1474, -2961, 0]);
    let extended = report.0[9..].to_vec();
    report.state(0x1000, 0, 0, &[0; 4]);
    assert_eq!(&report.0[9..], extended);
    report.motion(2, &[40000., -40000., f32::NAN]);
    assert_eq!(&report.0[12..18], &[255, 127, 0, 128, 0, 0]);
    report.advance_timestamp(Duration::from_nanos(5333 * 65535));
    report.advance_timestamp(Duration::from_nanos(5333 * 2));
    assert_eq!(&report.0[9..11], &[1, 0]);
}

#[test]
fn ds4_battery_clamps_out_of_range_client_percentages() {
    for percent in [251, 101, 254] {
        let mut report = Ds4Report::new();
        report.battery(3, percent);
        assert_eq!((report.0[11], report.0[29]), (255, 0x1a));
    }
}

#[test]
fn ds4_battery_scaling_and_unknown_values_match_vibepollo() {
    let mut report = Ds4Report::new();
    for (percent, scaled, level) in [
        (0, 0, 0x10),
        (50, 127, 0x15),
        (55, 140, 0x16),
        (100, 255, 0x1a),
    ] {
        report.battery(3, percent);
        assert_eq!((report.0[11], report.0[29]), (scaled, level));
    }
    report.battery(0, 255);
    assert_eq!((report.0[11], report.0[29]), (255, 0x1a));
    report.battery(4, 255);
    assert_eq!((report.0[11], report.0[29]), (255, 0x1b));
    report.battery(2, 50);
    assert_eq!((report.0[11], report.0[29]), (127, 5));
    for state in [1, 5] {
        report.battery(state, 25);
        assert_eq!((report.0[11], report.0[29]), (63, 0x1f));
    }
    report.battery(3, 255);
    assert_eq!(report.0[29], 0x15);
}

/// Vibepollo's keepalive: each report advances the timestamp by the time
/// since the last, a gyro silent past 750 ms reads as still, and the client
/// is asked for motion again after 2 s of silence, at most every 5 s.
#[test]
fn ds4_reports_advance_the_clock_and_still_a_silent_gyro() {
    let start = Instant::now();
    let mut ds4 = Ds4::new(start);
    ds4.report.motion(2, &[90., 0., 0.]);
    let packet = ds4.packet(0x0102_0304, start + Duration::from_millis(100));
    assert_eq!(&packet[..8], &[71, 0, 0, 0, 4, 3, 2, 1]);
    // 100 ms in 5.333 µs units.
    assert_eq!(u16::from_le_bytes([packet[17], packet[18]]), 18751);
    assert_ne!(&packet[20..26], &[0; 6], "a fresh gyro is kept");
    assert_eq!(&packet[8..], &ds4.report.0);
    let packet = ds4.packet(1, start + Duration::from_millis(800));
    assert_eq!(
        &packet[20..22],
        &1i16.to_le_bytes(),
        "x holds only the bias"
    );
    assert_eq!(&packet[22..26], &[0; 4]);
    assert!(!ds4.wants_motion(start + Duration::from_secs(4)));
    assert!(ds4.wants_motion(start + Duration::from_secs(6)));
    assert!(!ds4.wants_motion(start + Duration::from_secs(10)));
    assert!(ds4.wants_motion(start + Duration::from_millis(11_001)));
    ds4.gyro = start + Duration::from_secs(20);
    assert!(!ds4.wants_motion(start + Duration::from_secs(21)));
}

/// A slot this client has no pad in is an error: panic = abort would take the
/// host down, and a stranded slot of another backend is retried this way.
#[test]
#[ignore = "opens the installed ViGEmBus; plugs nothing"]
fn a_slot_without_a_pad_is_an_error_not_a_panic() -> Result<()> {
    let mut client = Client::open()?;
    assert!(client.unplug(3).is_err());
    assert!(client.feedback(3).is_err());
    assert!(client.submit(3, 0, 0, 0, &[0; 4]).is_err());
    Ok(())
}

#[repr(C)]
#[derive(Default)]
struct XInputState {
    packet: u32,
    buttons: u16,
    triggers: [u8; 2],
    sticks: [i16; 4],
}
type GetState = unsafe extern "system" fn(u32, *mut XInputState) -> u32;
type SetState = unsafe extern "system" fn(u32, *mut u16) -> u32;

/// What XInput shows on each of its four indices; None where nothing is plugged.
fn xinput(get: &GetState) -> [Option<u16>; 4] {
    std::array::from_fn(|index| {
        let mut state = XInputState::default();
        // SAFETY: `get` is XInputGetState from a library the caller holds, and
        // `state` matches the XINPUT_STATE it fills.
        (unsafe { get(index as u32, &mut state) } == 0).then_some(state.buttons)
    })
}

/// #74: the whole path through `Gamepads`, with what Windows then lists. Run by
/// hand with the host idle (serverinfo 0/0): it plugs a pad for a few seconds.
#[test]
#[ignore = "plugs one X360 on the installed ViGEmBus; needs an idle host"]
fn a_chosen_vigem_pad_reaches_xinput_as_a_usb_xbox_360_pad() -> Result<()> {
    let system = std::env::var("SystemRoot")?;
    // SAFETY: Windows' own XInput; it runs no code of concern on load.
    let library =
        unsafe { libloading::Library::new(format!("{system}\\System32\\xinput1_4.dll"))? };
    // SAFETY: the type is XInputGetState's documented signature.
    let get: libloading::Symbol<GetState> = unsafe { library.get(b"XInputGetState\0")? };
    let before = xinput(&get);
    let wait = |want: &dyn Fn(&[Option<u16>; 4]) -> bool| {
        (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            want(&xinput(&get))
        })
    };

    let mut pads = Gamepads::open(VIGEM_X360)?;
    let press = Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x1000,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    pads.apply(&press)?;
    let new = |now: &[Option<u16>; 4]| (0..4).find(|&i| before[i].is_none() && now[i].is_some());
    assert!(wait(&|now| new(now).is_some()), "no new XInput pad");
    let index = new(&xinput(&get)).unwrap();
    // A report sent before the Xbox driver reads the pad is dropped (the bus
    // answers ERROR_NO_MORE_ITEMS), as when Windows is still removing another
    // pad; a stream sends the state again, and so does this test.
    let mut reached = false;
    for _ in 0..100 {
        pads.apply(&press)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        if xinput(&get)[index] == Some(0x1000) {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "the A press never reached XInput: {:?}",
        xinput(&get)
    );
    // A second's look for `Get-PnpDevice -InstanceId 'USB\VID_045E&PID_028E*'`.
    let listed = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            r"Get-PnpDevice -PresentOnly -InstanceId 'USB\VID_045E&PID_028E*' | ForEach-Object InstanceId",
        ])
        .output()?;
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains(r"USB\VID_045E&PID_028E"), "{listed}");
    // A game's rumble comes back in the form the control stream forwards; the
    // bus also reports the motors off when the pad arrives.
    // SAFETY: the type is XInputSetState's documented signature.
    let set: libloading::Symbol<SetState> = unsafe { library.get(b"XInputSetState\0")? };
    let mut rumble = [0x8080u16, 0xffff];
    // SAFETY: `rumble` matches the XINPUT_VIBRATION it reads.
    assert_eq!(unsafe { set(index as u32, rumble.as_mut_ptr()) }, 0);
    let feedback = (0..100).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        pads.feedback()
            .into_iter()
            .find(|report| report.2.iter().any(|&byte| byte != 0))
    });
    assert_eq!(
        feedback,
        Some((0, 4, vec![0x80, 0x80, 0xff, 0xff, 0, 0, 0, 0]))
    );
    drop(pads);
    assert!(wait(&|now| now[index].is_none()), "the pad stayed plugged");
    Ok(())
}

/// One overlapped HID transfer on `device`, given up after `timeout`: the
/// bytes moved, or None when nothing came.
fn hid_io(
    device: HANDLE,
    write: Option<&[u8]>,
    read: Option<&mut [u8]>,
    timeout: u32,
) -> Result<Option<u32>> {
    let event = event()?;
    let mut overlapped = OVERLAPPED {
        hEvent: handle(&event),
        ..Default::default()
    };
    let mut moved = 0;
    // SAFETY: the buffer, `overlapped` and `event` outlive the transfer: every
    // path waits for it to complete or be cancelled before returning.
    unsafe {
        let started = match (write, read) {
            (Some(data), None) => WriteFile(device, Some(data), None, Some(&mut overlapped)),
            (None, Some(buffer)) => ReadFile(device, Some(buffer), None, Some(&mut overlapped)),
            _ => bail!("one HID transfer at a time"),
        };
        if let Err(error) = started
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.into());
        }
        if WaitForSingleObject(handle(&event), timeout) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(device, Some(&overlapped));
            let _ = GetOverlappedResult(device, &overlapped, &mut moved, true);
            return Ok(None);
        }
        GetOverlappedResult(device, &overlapped, &mut moved, false)?;
    }
    Ok(Some(moved))
}

/// #108: the whole path through `Gamepads` for the DualShock 4. Windows lists
/// a USB DS4, its HID input carries the state, a touch and the gyro, and a
/// lightbar colour written to it comes back once as feedback in the VHF DS4's
/// form, which the control stream sends as Moonlight's RGB LED message. Run by
/// hand with the host idle (serverinfo 0/0): it plugs a pad for a few seconds.
#[test]
#[ignore = "plugs one DS4 on the installed ViGEmBus; needs an idle host"]
fn a_chosen_vigem_ds4_is_a_usb_ds4_with_touch_motion_and_lightbar() -> Result<()> {
    use crate::input::native_touch_tests::{hid_paths, wait_removed};
    use butterpollo_core::input_policy::VIGEM_DS4;
    let before = hid_paths()?;
    let mut pads = Gamepads::open(VIGEM_DS4)?;
    // A PlayStation client with motion and a touchpad.
    pads.apply(&Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0x38,
        buttons: 0,
    })?;
    pads.apply(&Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x1000,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    })?;
    let path = (0..100)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            hid_paths().ok()?.into_iter().find(|path| {
                !before.contains(path) && path.to_ascii_lowercase().contains("vid_054c&pid_05c4")
            })
        })
        .context("no new DualShock 4 HID")?;
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is a terminated path that outlives the call; the handle
    // goes straight into an OwnedHandle.
    let hid = unsafe {
        OwnedHandle::from_raw_handle(
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                None,
            )?
            .0,
        )
    };
    let hid_handle = handle(&hid);
    // The newest input report that satisfies `want`, reading the queue while
    // the keepalive sends.
    let read_until = |pads: &mut Gamepads, want: &dyn Fn(&[u8]) -> bool| -> Result<[u8; 64]> {
        let mut report = [0u8; 64];
        for _ in 0..100 {
            pads.refresh()?;
            if hid_io(hid_handle, None, Some(&mut report), 200)? == Some(64)
                && report[0] == 1
                && want(&report)
            {
                return Ok(report);
            }
        }
        bail!("no matching DS4 input report; last {report:02x?}")
    };
    // Cross held, D-pad at rest; HID input is the DS4_REPORT_EX after report ID 1.
    read_until(&mut pads, &|report| report[5] == 0x28)?;

    pads.apply(&Event::ControllerTouch {
        id: 0,
        event: 1,
        touchpad: 0,
        pointer: 7,
        x: 0.5,
        y: 0.5,
        pressure: 1.,
    })?;
    let report = read_until(&mut pads, &|report| report[35] & 0x80 == 0)?;
    let x = u16::from(report[36]) | (u16::from(report[37] & 15) << 8);
    let y = u16::from(report[37] >> 4) | (u16::from(report[38]) << 4);
    assert_eq!((x, y), (960, 471), "touch at the centre: {report:02x?}");

    pads.apply(&Event::Motion {
        id: 0,
        kind: 2,
        xyz: [90., 0., 0.],
    })?;
    read_until(&mut pads, &|report| {
        i16::from_le_bytes([report[13], report[14]]) == 1474
    })?;

    // A tool sets the lightbar and rumble: USB output report 5.
    let mut output = [0u8; 32];
    output[..9].copy_from_slice(&[0x05, 0x07, 0x04, 0, 0x40, 0x80, 12, 34, 56]);
    assert_eq!(hid_io(hid_handle, Some(&output), None, 2000)?, Some(32));
    let lit = (0..100).find_map(|_| {
        std::thread::sleep(Duration::from_millis(50));
        pads.feedback()
            .into_iter()
            .find(|report| report.1 == 5 && report.2.get(4..7) == Some(&[12, 34, 56][..]))
    });
    assert_eq!(
        lit,
        Some((0, 5, vec![0x80, 0x80, 0x40, 0x40, 12, 34, 56, 1]))
    );
    // Rumble alone, same colour: the colour is not sent again.
    output[4..6].copy_from_slice(&[0, 0]);
    assert_eq!(hid_io(hid_handle, Some(&output), None, 2000)?, Some(32));
    let stopped = (0..100).find_map(|_| {
        std::thread::sleep(Duration::from_millis(50));
        pads.feedback()
            .into_iter()
            .find(|report| report.1 == 5 && report.2.get(..4) == Some(&[0; 4][..]))
    });
    assert_eq!(stopped, Some((0, 5, vec![0, 0, 0, 0, 12, 34, 56, 0])));

    // Windows lists it as a USB DualShock 4.
    let listed = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            r"Get-PnpDevice -PresentOnly -InstanceId 'USB\VID_054C&PID_05C4*' | ForEach-Object InstanceId",
        ])
        .output()?;
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains(r"USB\VID_054C&PID_05C4"), "{listed}");
    drop(hid);
    drop(pads);
    wait_removed(&path)?;
    Ok(())
}
