//! The contract between the Rubylight host and the Rubylight virtual display driver.
//!
//! Based on punktfunk's `pf-driver-proto` by Enrico Bühler (unom), MIT OR Apache-2.0
//! (<https://github.com/unom/punktfunk>). Identity, IOCTL numbering and the video transport
//! are Rubylight's own, so this driver never answers punktfunk's or libvirtualdisplay's host.
//!
//! Two planes:
//! * [`control`]: `DeviceIoControl` on the driver's device interface. Create, remove and
//!   re-mode virtual monitors, keep a lease alive, pin the render GPU and hand over a frame
//!   ring. Every struct has a fixed little-endian layout encoded by hand, so a one-sided edit
//!   fails the round-trip tests instead of corrupting a request.
//! * [`ring`]: the frame push. The host creates the shared textures, a small header section,
//!   an event and two shared fences, duplicates them into the driver's WUDFHost and hands
//!   their values over with [`control::SetFenceRingRequest`]. The driver copies every composed
//!   frame into a free texture, signals the written fence, stamps the present QPC into the
//!   header and signals the event; the host signals the released fence once it is done with a
//!   texture. The header's per-slot state words say who holds which texture. Pixels never
//!   leave the GPU. Hosts from before [`control::CAP_FENCE_RING`] sent the older keyed-mutex
//!   ring ([`control::IOCTL_SET_FRAME_RING`]), which this driver refuses.
//!
//! [`modes`] and [`edid`] hold the arithmetic the driver advertises to Windows. They live here
//! because the driver only builds under the WDK, and this crate runs `cargo test` anywhere.
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod control;
pub mod edid;
pub mod modes;
pub mod ring;

// Exercise the driver's actual GPU-independent retry policy in ordinary workspace CI,
// without linking the WDK driver. This adds no host-driver wire format or public API.
#[cfg(test)]
#[path = "../../rubylight-vdisplay/driver/src/frame_retry.rs"]
mod frame_retry;
#[cfg(test)]
#[path = "../../rubylight-vdisplay/driver/src/lifecycle.rs"]
mod lifecycle;

/// Device interface GUID `{5e240e6b-b196-4295-81c3-533c10000c59}` the driver registers and
/// the host opens. Not libvirtualdisplay's `{5f894d6c-...}` or punktfunk's `{70667664-...}`.
pub const INTERFACE_GUID_U128: u128 = 0x5e24_0e6b_b196_4295_81c3_533c_1000_0c59;

/// `(Data1, Data2, Data3, Data4)` of [`INTERFACE_GUID_U128`]; this crate has no `GUID` type.
#[must_use]
pub const fn interface_guid_fields() -> (u32, u16, u16, [u8; 8]) {
    guid_fields(INTERFACE_GUID_U128)
}

/// Split a GUID written as one hexadecimal number into its four fields.
#[must_use]
pub const fn guid_fields(g: u128) -> (u32, u16, u16, [u8; 8]) {
    (
        (g >> 96) as u32,
        (g >> 80) as u16,
        (g >> 64) as u16,
        (g as u64).to_be_bytes(),
    )
}

/// The root-enumerated hardware ID the INF binds and `devcon install` creates.
pub const HARDWARE_ID: &str = r"Root\RubylightVDisplay";

/// Bumped on any incompatible change to either plane. Host and driver ship together, so the
/// host refuses a driver whose [`control::InfoReply::protocol_version`] differs.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_guid_is_our_own() {
        const LIBVIRTUALDISPLAY: u128 = 0x5f89_4d6c_3a69_48a2_86ef_e4c6_7193_2d63;
        const PUNKTFUNK: u128 = 0x7066_7664_7044_5350_a1b2_c3d4_e5f6_0001;
        const SUDOVDA: u128 = 0xe5bc_c234_1e0c_418a_a0d4_ef8b_7501_414d;
        for other in [LIBVIRTUALDISPLAY, PUNKTFUNK, SUDOVDA] {
            assert_ne!(INTERFACE_GUID_U128, other);
        }
        assert_eq!(
            interface_guid_fields(),
            (
                0x5e24_0e6b,
                0xb196,
                0x4295,
                [0x81, 0xc3, 0x53, 0x3c, 0x10, 0x00, 0x0c, 0x59]
            )
        );
    }
}
