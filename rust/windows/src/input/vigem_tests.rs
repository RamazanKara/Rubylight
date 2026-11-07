use super::*;
use crate::input::{Event, gamepads::Gamepads};
use butterpollo_core::input_policy::VIGEM_X360;

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
        }),
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
fn notifications_use_a_vhf_xbox_pads_rumble_format() {
    assert_eq!(
        feedback_report(&[12, 0, 0, 0, 7, 0, 0, 0, 255, 128, 3, 0]),
        (4, vec![255, 255, 128, 128, 0, 0, 0, 0])
    );
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
    pads.apply(&Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x1000,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    })?;
    let new = |now: &[Option<u16>; 4]| (0..4).find(|&i| before[i].is_none() && now[i].is_some());
    assert!(wait(&|now| new(now).is_some()), "no new XInput pad");
    let index = new(&xinput(&get)).unwrap();
    assert!(
        wait(&|now| now[index] == Some(0x1000)),
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
