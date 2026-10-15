//! The platform seams for virtual displays the host creates itself and the frames their
//! drivers push to it. Windows implements them with Rubylight's IddCx driver
//! (`rubylight-vdisplay`, whose frame ring is `idd-push`); nothing here names a
//! Windows type, so another platform's virtual output can plug into the same places.

/// A display mode: pixels and refresh in millihertz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl Mode {
    #[must_use]
    pub const fn new(width: u32, height: u32, refresh_mhz: u32) -> Self {
        Self {
            width,
            height,
            refresh_mhz,
        }
    }
}

/// A driver that creates virtual displays and keeps them alive. Creation returns a
/// platform-specific reply the backend resolves into its display; the lifecycle after that
/// is the same for every backend.
pub trait DisplayProvider {
    /// Keep display `id` alive. An error means the driver no longer has it.
    fn renew(&self, id: u64) -> anyhow::Result<()>;
    /// Offer `mode` on the live display before the system is asked to set it. Drivers with a
    /// fixed mode list ignore it.
    fn offer_mode(&self, id: u64, mode: Mode);
    /// Remove the display; best effort, as a display whose heartbeats stop departs too.
    fn remove(&self, id: u64);
    /// Whether the driver pushes the display's frames itself ([`PushedFrames`]), so no
    /// separate screen capture is needed.
    fn pushes_frames(&self) -> bool;
}

/// One frame a display driver pushed: its publish sequence, the system's frame number, and
/// when it was presented, in the platform's monotonic ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushedFrame {
    pub seq: u64,
    pub frame_number: u64,
    /// When the driver received the composed frame.
    pub acquired: u64,
    /// The scan-out time the system attached, if any; `0` when absent.
    pub scanout: u64,
}

impl PushedFrame {
    /// The frame's present stamp. A scan-out stamp is normally later than the driver's
    /// receipt (IddCx's `PresentDisplayQPCTime`); then, or without one, the receipt is the
    /// present, the moment the frame was composed and handed over.
    #[must_use]
    pub const fn presented(&self) -> u64 {
        if self.scanout != 0 && (self.acquired == 0 || self.scanout <= self.acquired) {
            self.scanout
        } else {
            self.acquired
        }
    }
}

/// The host's side of a frame push: take the newest published frame without waiting. The
/// image holds the driver's buffer until it is dropped (the consumed/released handshake);
/// the driver never waits for it and drops or overwrites frames instead.
pub trait PushedFrames {
    type Image;
    /// The newest frame not yet taken, if one is published and a buffer can be held.
    fn take_newest(&mut self) -> anyhow::Result<Option<(PushedFrame, Self::Image)>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scanout_stamp_after_the_receipt_gives_way_to_the_receipt() {
        let frame = |scanout, acquired| PushedFrame {
            seq: 1,
            frame_number: 1,
            acquired,
            scanout,
        };
        // IddCx stamps the scan-out, after the acquire: the capture stage read zero.
        assert_eq!(frame(1_050, 1_000).presented(), 1_000);
        assert_eq!(frame(990, 1_000).presented(), 990);
        assert_eq!(frame(0, 1_000).presented(), 1_000);
        assert_eq!(frame(1_000, 0).presented(), 1_000);
    }
}
