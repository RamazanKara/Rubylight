//! Frame Warp 2's events on a stream's [`FrameWarpState`]: the gyro's motion packets, which the
//! control thread decodes, and whether the host's pointer shows, which the video thread reads
//! off each capture. Neither waits for the host: each only queues (applied unless someone holds
//! the host), and the control thread drops every motion packet but the gyro readings of the pad
//! the request named.

use std::sync::atomic::Ordering;
use std::time::Instant;

use butterpollo_media_policy::frame_warp::RequestV2;
use butterpollo_media_policy::frame_warp::gyro::GYRO_KIND;

use super::{Event, FrameWarpState};

/// No pad's gyro turns the camera.
pub(super) const NO_PAD: u8 = u8::MAX;
/// The pointer's visibility as last queued: not since the last request, hidden, shown.
pub(super) const CURSOR_UNKNOWN: u8 = 0;
const CURSOR_HIDDEN: u8 = 1;
const CURSOR_SHOWN: u8 = 2;

impl FrameWarpState {
    /// A request's gyro pad, for the control thread's filter; none once Frame Warp stops. The
    /// host's state may start over with a request, so the pointer is told again.
    pub(super) fn set_gyro(&self, request: &RequestV2) {
        let pad = request
            .gyro_pad()
            .filter(|_| request.base.enabled)
            .unwrap_or(NO_PAD);
        self.gyro_pad.store(pad, Ordering::Release);
        self.cursor.store(CURSOR_UNKNOWN, Ordering::Release);
    }

    /// A motion packet the control thread decoded at `at`: queued only for a gyro reading of
    /// the pad whose gyro turns the camera.
    pub fn on_motion(&self, number: u8, kind: u8, xyz: [f32; 3], at: Instant) {
        if kind != GYRO_KIND || !self.enabled() || self.gyro_pad.load(Ordering::Acquire) != number {
            return;
        }
        let at_us = self.us(at);
        self.push_quietly(Event::Motion {
            number,
            kind,
            xyz,
            at_us,
        });
    }

    /// Whether the host's pointer shows in a capture: a menu or a pause, which drops the camera
    /// confidence. Only a change is queued.
    pub fn on_cursor(&self, visible: bool) {
        if !self.enabled() {
            return;
        }
        let state = if visible { CURSOR_SHOWN } else { CURSOR_HIDDEN };
        if self.cursor.swap(state, Ordering::AcqRel) != state {
            self.push(Event::Cursor(visible));
        }
    }

    /// Whether this is the first request this stream ignored for not being version 2 (an older
    /// Nightjar's): it is logged once.
    pub fn first_ignored(&self) -> bool {
        !self.ignored.swap(true, Ordering::AcqRel)
    }
}
