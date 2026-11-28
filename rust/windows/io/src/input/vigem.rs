// ViGEmClient's DeviceIoControl protocol, ported to Rust from ViGEmClient.cpp,
// Common.h and km/BusShared.h (MIT). Copyright (c) 2016-2023 Nefarius Software
// Solutions e.U. and Contributors; see rust/THIRD_PARTY.md for the notice.
// The DualShock 4 report, its keepalive, the stale gyro and the lightbar
// follow Vibepollo's Windows input code (Nonary/Vibepollo, GPL-3.0), which
// carries LizardByte Sunshine's (GPL-3.0).
//! The optional ViGEmBus pads, each chosen by name: an Xbox 360 pad
//! (`gamepad = vigem_x360`, #74) and a DualShock 4 with touchpad, motion,
//! battery and lightbar (`gamepad = vigem_ds4`, #108). Both are emulated USB
//! devices, which tools that look for USB controllers see and a VHF pad they
//! may not. ViGEmBus is retired upstream and separately installed.
use anyhow::{Context, Result, bail};
use butterpollo_input_policy::input_policy::VIGEM_DS4;
use std::{
    collections::BTreeMap,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::Instant,
};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::*,
        Foundation::*,
        Storage::FileSystem::*,
        System::{IO::*, Threading::*},
    },
    core::{GUID, PCWSTR},
};

mod ds4;
#[cfg(test)]
use ds4::Ds4Report;
use ds4::{DS4_KEEPALIVE, Ds4};

const BUS: GUID = GUID::from_u128(0x96e42b22_f5e9_42f8_b043_ed0f932f014f);
const fn ioctl(function: u32, access: u32) -> u32 {
    (0x2a << 16) | (access << 14) | (function << 2)
}
const PLUGIN: u32 = ioctl(0x801, 2);
const UNPLUG: u32 = ioctl(0x802, 2);
const CHECK_VERSION: u32 = ioctl(0x803, 2);
const WAIT_READY: u32 = ioctl(0x804, 2);
const XUSB_NOTIFICATION: u32 = ioctl(0xa01, 3);
const XUSB_SUBMIT: u32 = ioctl(0xa02, 2);
const DS4_SUBMIT: u32 = ioctl(0xa03, 2);
// Write-only access, unlike the XUSB notification: the bus routes only this code.
const DS4_NOTIFICATION: u32 = ioctl(0xa04, 2);

/// The two ViGEmBus target types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    X360,
    Ds4,
}
impl Kind {
    /// The notification request's code and size, which the bus answers in
    /// place: XUSB_REQUEST_NOTIFICATION or DS4_REQUEST_NOTIFICATION.
    fn notification(self) -> (u32, u32) {
        match self {
            Self::X360 => (XUSB_NOTIFICATION, 12),
            Self::Ds4 => (DS4_NOTIFICATION, 16),
        }
    }
}

fn handle(value: &OwnedHandle) -> HANDLE {
    HANDLE(value.as_raw_handle())
}
fn event() -> Result<OwnedHandle> {
    // SAFETY: CreateEventW returned a new event handle, which only this OwnedHandle owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(CreateEventW(None, true, false, None)?.0) })
}
fn packet(size: usize, serial: u32) -> Vec<u8> {
    let mut data = vec![0; size];
    data[..4].copy_from_slice(&(size as u32).to_le_bytes());
    data[4..8].copy_from_slice(&serial.to_le_bytes());
    data
}

const REPORT_TIMEOUT_MS: u32 = 2_000;
const CONTROL_TIMEOUT_MS: u32 = 5_000;
const CANCEL_WAIT_MS: u32 = 200;

struct PendingIo(OVERLAPPED, #[allow(dead_code)] Vec<u8>);
// SAFETY: `PendingIo` is only moved to a reaper thread while the kernel still owns an abandoned
// async IOCTL buffer after `CancelIoEx`; nothing accesses its fields until `WaitForSingleObject`
// on the associated event finishes.
unsafe impl Send for PendingIo {}

/// Hands an in-flight timed-out overlapped IO buffer and replaced event handle
/// to a reaper thread; if spawning fails (or replacing the event fails), retries
/// `cancel_and_wait` synchronously and leaks any still-in-flight `pending`/`stale`
/// rather than freeing memory or closing a handle the kernel IRP still references.
fn abandon_io_with<P: Send + 'static>(
    pending: P,
    event: &mut OwnedHandle,
    cancel_and_wait: impl FnOnce(&P, HANDLE) -> bool,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) {
    if let Ok(fresh) = self::event() {
        let stale = std::mem::replace(event, fresh);
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some((pending, stale))));
        let worker_slot = slot.clone();
        let spawned = spawn(Box::new(move || {
            if let Some((pending, stale)) = worker_slot.lock().unwrap().take() {
                // SAFETY: `stale` is owned by this thread until dropped below after completion.
                let _ = unsafe { WaitForSingleObject(handle(&stale), INFINITE) };
                drop(pending);
                drop(stale);
            }
        }));
        if let Err(error) = spawned
            && let Some((pending, stale)) = slot.lock().unwrap().take()
            && !cancel_and_wait(&pending, handle(&stale))
        {
            tracing::warn!(
                %error,
                "could not spawn ViGEm cancel thread; retaining in-flight IO buffer and event"
            );
            std::mem::forget(pending);
            std::mem::forget(stale);
        }
    } else if !cancel_and_wait(&pending, handle(event)) {
        std::mem::forget(pending);
    }
}

fn control(bus: HANDLE, code: u32, data: Vec<u8>) -> Result<()> {
    let mut ev = event()?;
    control_with(bus, &mut ev, code, data, CONTROL_TIMEOUT_MS)
}
/// One request on `event`, which the request resets as it starts; the
/// client reuses one for every report instead of creating one each time.
fn control_with(
    bus: HANDLE,
    event: &mut OwnedHandle,
    code: u32,
    data: Vec<u8>,
    timeout_ms: u32,
) -> Result<()> {
    let mut pending = Box::new(PendingIo(
        OVERLAPPED {
            hEvent: handle(event),
            ..Default::default()
        },
        data,
    ));
    let mut transferred = 0;
    // SAFETY: `pending` lives on the heap for the entire IRP; if cancellation does not finish
    // within `CANCEL_WAIT_MS`, ownership of both `pending` and the replaced event handle moves to
    // a detached waiter thread (or is leaked after a retry if spawn fails) rather than blocking
    // the caller or freeing in-flight kernel buffers.
    unsafe {
        let started = DeviceIoControl(
            bus,
            code,
            Some(pending.1.as_ptr().cast()),
            pending.1.len() as u32,
            None,
            0,
            None,
            Some(&mut pending.0),
        );
        if let Err(error) = started
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.into());
        }
        if WaitForSingleObject(handle(event), timeout_ms) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(bus, Some(&pending.0));
            if WaitForSingleObject(handle(event), CANCEL_WAIT_MS) == WAIT_OBJECT_0 {
                if GetOverlappedResult(bus, &pending.0, &mut transferred, false).is_ok() {
                    return Ok(());
                }
                bail!("ViGEmBus request {code:#x} timed out");
            }
            abandon_io_with(
                pending,
                event,
                |pending, stale| {
                    let _ = CancelIoEx(bus, Some(&pending.0));
                    WaitForSingleObject(stale, CANCEL_WAIT_MS) == WAIT_OBJECT_0
                },
                |task| {
                    std::thread::Builder::new()
                        .name("vigem-cancel".into())
                        .spawn(task)
                        .map(|_| ())
                },
            );
            bail!("ViGEmBus request {code:#x} timed out");
        }
        GetOverlappedResult(bus, &pending.0, &mut transferred, false)?;
    }
    Ok(())
}

fn report_result(result: Result<()>) -> Result<()> {
    match result {
        // A ready target may not have a USB read queued yet. ViGEmClient also
        // accepts this status.
        Err(error)
            if error
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|error| error.code() == ERROR_NO_MORE_ITEMS.to_hresult()) =>
        {
            Ok(())
        }
        other => other,
    }
}

fn open_bus() -> Result<OwnedHandle> {
    // SAFETY: the device information set lives until the destroy call below, and
    // `buffer` is sized and aligned for the detail structure the second call fills.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&BUS),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )?;
        let result = (|| {
            let mut last_error = anyhow::anyhow!("ViGEmBus is not installed");
            for index in 0.. {
                let mut interface = SP_DEVICE_INTERFACE_DATA {
                    cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                match SetupDiEnumDeviceInterfaces(set, None, &BUS, index, &mut interface) {
                    Ok(()) => {}
                    Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => break,
                    Err(error) => return Err(error.into()),
                }
                let opened = (|| -> Result<OwnedHandle> {
                    let mut size = 0;
                    let _ = SetupDiGetDeviceInterfaceDetailW(
                        set,
                        &interface,
                        None,
                        0,
                        Some(&mut size),
                        None,
                    );
                    if !(8..=65536).contains(&size) {
                        bail!("invalid ViGEmBus interface path length");
                    }
                    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                    let detail = buffer
                        .as_mut_ptr()
                        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                    (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                    SetupDiGetDeviceInterfaceDetailW(
                        set,
                        &interface,
                        Some(detail),
                        size,
                        None,
                        None,
                    )?;
                    let bus = OwnedHandle::from_raw_handle(
                        CreateFileW(
                            PCWSTR((*detail).DevicePath.as_ptr()),
                            GENERIC_READ.0 | GENERIC_WRITE.0,
                            FILE_SHARE_READ | FILE_SHARE_WRITE,
                            None,
                            OPEN_EXISTING,
                            FILE_ATTRIBUTE_NORMAL
                                | FILE_FLAG_OVERLAPPED
                                | FILE_FLAG_NO_BUFFERING
                                | FILE_FLAG_WRITE_THROUGH,
                            None,
                        )?
                        .0,
                    );
                    control(handle(&bus), CHECK_VERSION, packet(8, 1))
                        .context("incompatible ViGEmBus protocol")?;
                    Ok(bus)
                })();
                match opened {
                    Ok(bus) => return Ok(bus),
                    Err(error) => last_error = error,
                }
            }
            Err(last_error)
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        result
    }
}

struct Pending {
    overlapped: OVERLAPPED,
    data: [u8; 16],
}
struct Notification {
    bus: HANDLE,
    event: OwnedHandle,
    // Gamepads can move between polls; pending kernel pointers must not move.
    pending: Box<Pending>,
    running: bool,
    kind: Kind,
    /// The DualShock 4 lightbar colour last forwarded.
    lightbar: Option<[u8; 3]>,
}
impl Notification {
    fn new(bus: HANDLE, serial: u32, kind: Kind) -> Result<Self> {
        let event = event()?;
        let mut pending = Box::new(Pending {
            overlapped: OVERLAPPED::default(),
            data: [0; 16],
        });
        pending.data[..4].copy_from_slice(&kind.notification().1.to_le_bytes());
        pending.data[4..8].copy_from_slice(&serial.to_le_bytes());
        let mut notification = Self {
            bus,
            event,
            pending,
            running: false,
            kind,
            lightbar: None,
        };
        notification.start()?;
        Ok(notification)
    }
    fn start(&mut self) -> Result<()> {
        // SAFETY: the boxed buffer and OVERLAPPED stay where they are until the
        // request completes: `drop` cancels and waits for a running one.
        unsafe {
            ResetEvent(handle(&self.event))?;
            self.pending.overlapped = OVERLAPPED {
                hEvent: handle(&self.event),
                ..Default::default()
            };
            let (code, size) = self.kind.notification();
            let data = self.pending.data.as_mut_ptr();
            let result = DeviceIoControl(
                self.bus,
                code,
                Some(data.cast()),
                size,
                Some(data.cast()),
                size,
                None,
                Some(&mut self.pending.overlapped),
            );
            if let Err(error) = result
                && error.code() != ERROR_IO_PENDING.to_hresult()
            {
                return Err(error.into());
            }
        }
        self.running = true;
        Ok(())
    }
    /// The event of the running request, which the gamepad thread waits on.
    fn wait_handle(&self) -> Option<HANDLE> {
        self.running.then(|| handle(&self.event))
    }
    /// Whether the running request has completed, without a system call.
    fn completed(&self) -> bool {
        // SAFETY: the OVERLAPPED is alive while the request runs; the
        // kernel writes its status, so it is read as HasOverlappedIoCompleted does.
        self.running
            && unsafe { std::ptr::read_volatile(&self.pending.overlapped.Internal) }
                != STATUS_PENDING.0 as usize
    }
    fn poll(&mut self) -> Result<Option<(u16, Vec<u8>)>> {
        if !self.running {
            self.start()?;
        }
        let mut transferred = 0;
        // SAFETY: the OVERLAPPED of the running request is alive; no wait, so
        // an unfinished request only reports ERROR_IO_INCOMPLETE.
        let result = unsafe {
            GetOverlappedResult(self.bus, &self.pending.overlapped, &mut transferred, false)
        };
        if let Err(error) = &result
            && error.code() == ERROR_IO_INCOMPLETE.to_hresult()
        {
            return Ok(None);
        }
        self.running = false;
        result?;
        if transferred != self.kind.notification().1 {
            bail!("invalid ViGEmBus notification length");
        }
        let report = feedback_report(self.kind, &self.pending.data, &mut self.lightbar);
        // Re-armed at once: ViGEmBus holds what the game writes meanwhile
        // until the next request. A request that would not start keeps this
        // report; the next poll starts it again and returns its error.
        if let Err(error) = self.start() {
            tracing::debug!(%error, "ViGEm notification request did not start; retried at the next poll");
        }
        Ok(Some(report))
    }
}
impl Drop for Notification {
    fn drop(&mut self) {
        if self.running {
            // SAFETY: the wait returns only once the kernel is done with the
            // buffer and OVERLAPPED, which are freed after it.
            unsafe {
                let _ = CancelIoEx(self.bus, Some(&self.pending.overlapped));
                let _ = GetOverlappedResult(self.bus, &self.pending.overlapped, &mut 0, true);
            }
        }
    }
}

/// A notification as the VHF pad of the same family reports it
/// (`feedback_packets` in the host). An Xbox 360 pad's is kind 4: large and
/// small motor, then the two trigger motors it doesn't have. A DualShock 4's
/// is kind 5: the motors, the lightbar, and flag 1 when its colour changed.
/// The bus repeats the last colour with every rumble, so, as in Vibepollo, a
/// colour already forwarded is not sent again.
fn feedback_report(kind: Kind, data: &[u8], lightbar: &mut Option<[u8; 3]>) -> (u16, Vec<u8>) {
    // XUSB: large motor, then small; DS4_OUTPUT_REPORT: small, large, colour.
    let (large, small) = match kind {
        Kind::X360 => (data[8], data[9]),
        Kind::Ds4 => (data[9], data[8]),
    };
    let mut report = vec![0; 8];
    report[..2].copy_from_slice(&(u16::from(large) * 257).to_le_bytes());
    report[2..4].copy_from_slice(&(u16::from(small) * 257).to_le_bytes());
    if kind == Kind::X360 {
        return (4, report);
    }
    let colour = [data[10], data[11], data[12]];
    report[4..7].copy_from_slice(&colour);
    report[7] = u8::from(lightbar.replace(colour) != Some(colour));
    (5, report)
}

fn xusb_report(buttons: u32, left: u8, right: u8, sticks: &[i16; 4]) -> [u8; 12] {
    let mut report = [0; 12];
    let buttons = (buttons & 0xf7ff) as u16 | if buttons & 0x200000 != 0 { 0x400 } else { 0 };
    report[..2].copy_from_slice(&buttons.to_le_bytes());
    report[2..4].copy_from_slice(&[left, right]);
    for (offset, stick) in [4, 6, 8, 10].into_iter().zip(sticks) {
        report[offset..offset + 2].copy_from_slice(&stick.to_le_bytes());
    }
    report
}

/// XUSB_SUBMIT_REPORT, built on the stack: a report per controller state.
fn submit_packet(serial: u32, buttons: u32, left: u8, right: u8, sticks: &[i16; 4]) -> [u8; 20] {
    let mut data = [0; 20];
    data[..4].copy_from_slice(&20u32.to_le_bytes());
    data[4..8].copy_from_slice(&serial.to_le_bytes());
    data[8..].copy_from_slice(&xusb_report(buttons, left, right, sticks));
    data
}

struct Target {
    bus: HANDLE,
    serial: u32,
    notification: Option<Notification>,
    ds4: Option<Ds4>,
}
#[cfg(test)]
thread_local! {
    static UNPLUG_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Target {
    /// Sends a DualShock 4's report as it is at `now`, on `event`.
    fn send_ds4(&mut self, event: &mut OwnedHandle, now: Instant) -> Result<()> {
        let ds4 = self.ds4.as_mut().context("not a ViGEmBus DualShock 4")?;
        let data = ds4.packet(self.serial, now);
        report_result(control_with(
            self.bus,
            event,
            DS4_SUBMIT,
            data.to_vec(),
            REPORT_TIMEOUT_MS,
        ))
    }
    fn unplug(&mut self) -> Result<()> {
        #[cfg(test)]
        UNPLUG_ATTEMPTS.with(|count| count.set(count.get() + 1));
        control(self.bus, UNPLUG, packet(8, self.serial))?;
        self.notification = None;
        self.serial = 0;
        Ok(())
    }
}
impl Drop for Target {
    fn drop(&mut self) {
        if self.serial != 0
            && let Err(error) = self.unplug()
        {
            tracing::warn!(%error, serial = self.serial, "could not unplug ViGEm target");
        }
    }
}

pub(super) struct Client {
    // Targets cancel their pending IO and unplug before the bus handle closes.
    targets: BTreeMap<u32, Target>,
    /// The event every report waits on, one per client rather than per report.
    submitted: OwnedHandle,
    bus: OwnedHandle,
}
impl Client {
    pub(super) fn open() -> Result<Self> {
        Ok(Self {
            targets: BTreeMap::new(),
            submitted: event()?,
            bus: open_bus()?,
        })
    }
    /// A client without the driver, on a handle no request works on.
    #[cfg(test)]
    pub(super) fn detached() -> Result<Self> {
        Ok(Self {
            targets: BTreeMap::new(),
            submitted: event()?,
            bus: event()?,
        })
    }
    fn target(&mut self, slot: u32) -> Result<&mut Target> {
        self.parts(slot).map(|(target, _)| target)
    }
    /// A slot's target with the event its reports wait on.
    fn parts(&mut self, slot: u32) -> Result<(&mut Target, &mut OwnedHandle)> {
        let target = self
            .targets
            .get_mut(&slot)
            .with_context(|| format!("no ViGEmBus pad in slot {slot}"))?;
        Ok((target, &mut self.submitted))
    }
    pub(super) fn plug(&mut self, slot: u32, profile: u16) -> Result<()> {
        // One pad per slot: try to take the old target out before the new one
        // can take a player index beside it. A refused unplug is the driver's
        // to answer later; replacing the pad must still go on.
        if let Some(mut previous) = self.targets.remove(&slot)
            && let Err(error) = previous.unplug()
        {
            tracing::warn!(
                %error,
                slot,
                "could not unplug the previous ViGEm target; replacing it anyway"
            );
            // Prevent `Target::drop` from immediately retrying the same wedged unplug.
            previous.serial = 0;
        }
        let kind = if profile == VIGEM_DS4 {
            Kind::Ds4
        } else {
            Kind::X360
        };
        // Target type 0 is the Xbox 360 pad, as USB\VID_045E&PID_028E; type 2
        // the DualShock 4, as USB\VID_054C&PID_05C4.
        let (target_type, vendor, product) = match kind {
            Kind::X360 => (0u32, 0x045eu16, 0x028eu16),
            Kind::Ds4 => (2, 0x054c, 0x05c4),
        };
        let mut request = packet(16, 0);
        request[8..12].copy_from_slice(&target_type.to_le_bytes());
        request[12..14].copy_from_slice(&vendor.to_le_bytes());
        request[14..16].copy_from_slice(&product.to_le_bytes());
        let bus = handle(&self.bus);
        let mut last_error = anyhow::anyhow!("no free ViGEmBus serial numbers");
        for serial in 1..=u16::MAX as u32 {
            request[4..8].copy_from_slice(&serial.to_le_bytes());
            if let Err(error) = control(bus, PLUGIN, request.clone()) {
                // Bus_PlugInDevice uses INVALID_PARAMETER for a serial collision.
                if !error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|error| error.code() == ERROR_INVALID_PARAMETER.to_hresult())
                {
                    // A refused request may still have got as far as a pad.
                    if let Err(undo) = control(bus, UNPLUG, packet(8, serial)) {
                        tracing::debug!(%undo, serial, "no ViGEm target to take back");
                    }
                    return Err(error.context("could not plug ViGEm target"));
                }
                last_error = error;
                continue;
            }
            let mut target = Target {
                bus,
                serial,
                notification: None,
                ds4: None,
            };
            // Before v1.17 the plug request itself waited for readiness.
            if let Err(error) = control(bus, WAIT_READY, packet(8, serial))
                && !error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|error| error.code() == ERROR_INVALID_PARAMETER.to_hresult())
            {
                return Err(error.context("ViGEm target did not become ready"));
            }
            target.notification = Some(Notification::new(bus, serial, kind)?);
            if kind == Kind::Ds4 {
                let now = Instant::now();
                target.ds4 = Some(Ds4::new(now));
                // At rest from the start, held flat rather than in free fall.
                target.send_ds4(&mut self.submitted, now)?;
            }
            self.targets.insert(slot, target);
            return Ok(());
        }
        Err(last_error.context("could not plug ViGEm target"))
    }
    pub(super) fn unplug(&mut self, slot: u32) -> Result<()> {
        self.target(slot)?.unplug()?;
        self.targets.remove(&slot);
        Ok(())
    }
    pub(super) fn submit(
        &mut self,
        slot: u32,
        buttons: u32,
        left: u8,
        right: u8,
        sticks: &[i16; 4],
    ) -> Result<()> {
        let (target, submitted) = self.parts(slot)?;
        if let Some(ds4) = &mut target.ds4 {
            ds4.report.state(buttons, left, right, sticks);
            return target.send_ds4(submitted, Instant::now());
        }
        let data = submit_packet(target.serial, buttons, left, right, sticks);
        report_result(control_with(
            target.bus,
            submitted,
            XUSB_SUBMIT,
            data.to_vec(),
            REPORT_TIMEOUT_MS,
        ))
    }
    /// Moonlight motion for a DualShock 4; an Xbox 360 pad has no sensors.
    pub(super) fn motion(&mut self, slot: u32, kind: u8, xyz: &[f32; 3]) -> Result<()> {
        let (target, submitted) = self.parts(slot)?;
        let Some(ds4) = &mut target.ds4 else {
            return Ok(());
        };
        let now = Instant::now();
        ds4.report.motion(kind, xyz);
        if kind == 2 {
            ds4.gyro = now;
        }
        target.send_ds4(submitted, now)
    }
    /// A touch on a DualShock 4's touchpad. One that doesn't reach the pad
    /// leaves the report as it was, as the gamepads' contact map does.
    pub(super) fn touch(
        &mut self,
        slot: u32,
        contact: u8,
        event: u8,
        position: [f32; 2],
    ) -> Result<()> {
        let (target, submitted) = self.parts(slot)?;
        let Some(ds4) = &mut target.ds4 else {
            return Ok(());
        };
        let previous = ds4.report.clone();
        ds4.report.touch(contact, event, position);
        let sent = target.send_ds4(submitted, Instant::now());
        if sent.is_err()
            && let Some(ds4) = &mut target.ds4
        {
            ds4.report = previous;
        }
        sent
    }
    pub(super) fn battery(&mut self, slot: u32, state: u8, percent: u8) -> Result<()> {
        let (target, submitted) = self.parts(slot)?;
        let Some(ds4) = &mut target.ds4 else {
            return Ok(());
        };
        ds4.report.battery(state, percent);
        target.send_ds4(submitted, Instant::now())
    }
    /// Sends each DualShock 4's report again once it is [`DS4_KEEPALIVE`]
    /// old. Every pad is tried; the last failure is returned.
    pub(super) fn refresh(&mut self) -> Result<()> {
        let now = Instant::now();
        let mut result = Ok(());
        for target in self.targets.values_mut() {
            if target
                .ds4
                .as_ref()
                .is_some_and(|ds4| now.saturating_duration_since(ds4.sent) >= DS4_KEEPALIVE)
                && let Err(error) = target.send_ds4(&mut self.submitted, now)
            {
                result = Err(error);
            }
        }
        result
    }
    /// The slots of DualShock 4s whose client should be asked for motion
    /// again: no gyro reading for [`DS4_MOTION_REQUEST_AFTER`].
    pub(super) fn motion_requests(&mut self) -> Vec<u32> {
        let now = Instant::now();
        self.targets
            .iter_mut()
            .filter_map(|(slot, target)| {
                target
                    .ds4
                    .as_mut()
                    .is_some_and(|ds4| ds4.wants_motion(now))
                    .then_some(*slot)
            })
            .collect()
    }
    pub(super) fn wait_handle(&self, slot: u32) -> Option<HANDLE> {
        self.targets
            .get(&slot)?
            .notification
            .as_ref()?
            .wait_handle()
    }
    pub(super) fn feedback_ready(&self, slot: u32) -> bool {
        self.targets
            .get(&slot)
            .and_then(|target| target.notification.as_ref())
            .is_some_and(Notification::completed)
    }
    pub(super) fn feedback(&mut self, slot: u32) -> Result<Option<(u16, Vec<u8>)>> {
        self.target(slot)?
            .notification
            .as_mut()
            .map(Notification::poll)
            .transpose()
            .map(Option::flatten)
    }
}

#[cfg(test)]
#[path = "vigem_tests.rs"]
mod tests;
