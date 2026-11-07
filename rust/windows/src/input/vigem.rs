// ViGEmClient's DeviceIoControl protocol, ported to Rust from ViGEmClient.cpp,
// Common.h and km/BusShared.h (MIT). Copyright (c) 2016-2023 Nefarius Software
// Solutions e.U. and Contributors; see rust/THIRD_PARTY.md for the notice.
//! The optional ViGEmBus Xbox 360 pad (`gamepad = vigem_x360`, #74): an emulated
//! USB device, which tools that look for USB controllers see and a VHF pad
//! they may not. ViGEmBus is retired upstream and separately installed.
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
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
const NOTIFICATION_SIZE: u32 = 12;

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

fn control(bus: HANDLE, code: u32, data: &[u8]) -> Result<()> {
    let event = event()?;
    let mut overlapped = OVERLAPPED {
        hEvent: handle(&event),
        ..Default::default()
    };
    let mut transferred = 0;
    // SAFETY: `data`, `overlapped` and `event` outlive the request: every path
    // leaves only after it completed, a timeout through the blocking wait below.
    unsafe {
        let started = DeviceIoControl(
            bus,
            code,
            Some(data.as_ptr().cast()),
            data.len() as u32,
            None,
            0,
            None,
            Some(&mut overlapped),
        );
        if let Err(error) = started
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.into());
        }
        if WaitForSingleObject(handle(&event), 10_000) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(bus, Some(&overlapped));
            // Cancellation is asynchronous: the kernel must release the input
            // buffer and OVERLAPPED before their storage goes away.
            let _ = GetOverlappedResult(bus, &overlapped, &mut transferred, true);
            bail!("ViGEmBus request {code:#x} timed out");
        }
        GetOverlappedResult(bus, &overlapped, &mut transferred, false)?;
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
                    control(handle(&bus), CHECK_VERSION, &packet(8, 1))
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
}
impl Notification {
    fn new(bus: HANDLE, serial: u32) -> Result<Self> {
        let event = event()?;
        let mut pending = Box::new(Pending {
            overlapped: OVERLAPPED::default(),
            data: [0; 16],
        });
        pending.data[..4].copy_from_slice(&NOTIFICATION_SIZE.to_le_bytes());
        pending.data[4..8].copy_from_slice(&serial.to_le_bytes());
        let mut notification = Self {
            bus,
            event,
            pending,
            running: false,
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
            let data = self.pending.data.as_mut_ptr();
            let result = DeviceIoControl(
                self.bus,
                XUSB_NOTIFICATION,
                Some(data.cast()),
                NOTIFICATION_SIZE,
                Some(data.cast()),
                NOTIFICATION_SIZE,
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
        if transferred != NOTIFICATION_SIZE {
            bail!("invalid ViGEmBus notification length");
        }
        let report = feedback_report(&self.pending.data);
        self.start()?;
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

/// Rumble as a VHF Xbox pad reports it: kind 4, then large and small motor and
/// the two trigger motors this pad doesn't have (`feedback_packets` in the host).
fn feedback_report(data: &[u8]) -> (u16, Vec<u8>) {
    let (large, small) = (data[8], data[9]);
    let mut report = vec![0; 8];
    // Use the same 8-to-16 bit scaling as Vibepollo's ViGEm callbacks.
    report[..2].copy_from_slice(&(u16::from(large) * 257).to_le_bytes());
    report[2..4].copy_from_slice(&(u16::from(small) * 257).to_le_bytes());
    (4, report)
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

struct Target {
    bus: HANDLE,
    serial: u32,
    notification: Option<Notification>,
}
impl Target {
    fn unplug(&mut self) -> Result<()> {
        control(self.bus, UNPLUG, &packet(8, self.serial))?;
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
    bus: OwnedHandle,
}
impl Client {
    pub(super) fn open() -> Result<Self> {
        Ok(Self {
            targets: BTreeMap::new(),
            bus: open_bus()?,
        })
    }
    fn target(&mut self, slot: u32) -> Result<&mut Target> {
        self.targets
            .get_mut(&slot)
            .with_context(|| format!("no ViGEmBus pad in slot {slot}"))
    }
    pub(super) fn plug(&mut self, slot: u32) -> Result<()> {
        let mut request = packet(16, 0);
        // Target type 0 is the Xbox 360 pad, as USB\VID_045E&PID_028E.
        request[12..14].copy_from_slice(&0x045eu16.to_le_bytes());
        request[14..16].copy_from_slice(&0x028eu16.to_le_bytes());
        let bus = handle(&self.bus);
        let mut last_error = anyhow::anyhow!("no free ViGEmBus serial numbers");
        for serial in 1..=u16::MAX as u32 {
            request[4..8].copy_from_slice(&serial.to_le_bytes());
            if let Err(error) = control(bus, PLUGIN, &request) {
                // Bus_PlugInDevice uses INVALID_PARAMETER for a serial collision.
                if !error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|error| error.code() == ERROR_INVALID_PARAMETER.to_hresult())
                {
                    return Err(error.context("could not plug ViGEm target"));
                }
                last_error = error;
                continue;
            }
            let mut target = Target {
                bus,
                serial,
                notification: None,
            };
            // Before v1.17 the plug request itself waited for readiness.
            if let Err(error) = control(bus, WAIT_READY, &packet(8, serial))
                && !error
                    .downcast_ref::<windows::core::Error>()
                    .is_some_and(|error| error.code() == ERROR_INVALID_PARAMETER.to_hresult())
            {
                return Err(error.context("ViGEm target did not become ready"));
            }
            target.notification = Some(Notification::new(bus, serial)?);
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
        let target = self.target(slot)?;
        let mut data = packet(20, target.serial);
        data[8..].copy_from_slice(&xusb_report(buttons, left, right, sticks));
        report_result(control(target.bus, XUSB_SUBMIT, &data))
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
