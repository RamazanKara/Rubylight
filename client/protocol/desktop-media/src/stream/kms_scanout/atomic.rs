//! Atomic KMS plane engagement and property-set builders for [`Scanout`].

use nightjar_core::control::HdrMetadata;

use crate::kms::drm::{self, Card, Framebuffer, ModeInfo, Props};
use crate::kms::plan::{self, Layout, Object, Plane, Value};

use super::{caps_of, picture_rect, Engaged, OverlayPlane, Scanout, Viewport, Wishes};

impl Scanout {
    /// The first picture (its framebuffer `fb`): the card's state now, the layout a test commit
    /// takes, the display mode for the stream's rate and the stream's properties, in one commit
    /// that shows the picture.
    pub(super) fn engage(
        &mut self,
        layout: plan::FbLayout,
        fb: Framebuffer,
        (surface, generation): (u32, crate::dmabuf::Generation),
        visible: (i32, i32),
        (hdr, color): (Option<HdrMetadata>, (i32, i32)),
        viewport: &Viewport<'_>,
    ) -> Result<(), String> {
        let snapshot = match self.read_state() {
            Ok(snapshot) => snapshot,
            Err(why) => {
                self.card.remove_fb(fb);
                return Err(why);
            }
        };
        let card = &self.card;
        let plain: Vec<Plane> = snapshot.planes.iter().map(|(p, _)| p.clone()).collect();
        let layouts: Vec<Layout> = plan::candidates(&plain, self.output.crtc_index, layout.format, layout.modifier)
            .into_iter()
            .filter(|l| l.overlay.is_some())
            .collect();
        if layouts.is_empty() {
            card.remove_fb(fb);
            return Err(format!(
                "no two planes of {} take {} pictures with modifier {:#x} and an overlay above them",
                self.output.name,
                plan::fourcc_name(layout.format),
                layout.modifier
            ));
        }
        let (w, h) = (u32::from(snapshot.mode.hdisplay), u32::from(snapshot.mode.vdisplay));
        let overlay_buffers = match (0..2).map(|_| card.dumb_buffer(w, h, plan::ARGB8888)).collect::<std::io::Result<Vec<_>>>() {
            Ok(buffers) => buffers,
            Err(e) => {
                card.remove_fb(fb);
                return Err(format!("no overlay buffers: {e}"));
            }
        };
        let rect = picture_rect(&snapshot.mode, &layout, visible, viewport);
        let next_fb = fb.id;
        let vrr_on = plan::vrr_engaged(&caps_of(&snapshot.connector, &snapshot.crtc), self.wishes.vrr);
        let (fmt, modifier) = (layout.format, layout.modifier);
        let mut engaged = Engaged {
            snapshot, layout: layouts[0].clone(), format: (fmt, modifier), generation: Some(generation),
            fbs: vec![(surface, fb, layout)], retired: Vec::new(), next_fb, shown_fb: 0, rect, shown_rect: None,
            on_screen: None,
            overlay: OverlayPlane { plane: 0, buffers: overlay_buffers, front: 0, shown: false, change: None, last_canvas: Vec::new() },
            background: None, hdr, color, blobs: Vec::new(), restore: Vec::new(), props_dirty: true, vrr_on,
        };
        // The display mode for the stream's rate (or top refresh when VRR is engaged), of the size in use.
        let plan_modes: Vec<plan::Mode> = engaged.snapshot.modes.iter().map(ModeInfo::plan).collect();
        let current = engaged.snapshot.modes.iter().position(|m| *m == engaged.snapshot.mode);
        let wanted = plan::pick_mode(&plan_modes, current, self.wishes.fps, engaged.vrr_on).map(|i| engaged.snapshot.modes[i]);
        let original_mode = engaged.snapshot.mode;
        let mode_blob = match wanted.filter(|m| *m != engaged.snapshot.mode) {
            Some(m) => match card.mode_blob(&m) {
                Ok(blob) => {
                    eprintln!("nightjar: kms: display mode {}x{} at {:.2} Hz for {} fps", m.hdisplay, m.vdisplay, m.refresh_hz(), self.wishes.fps);
                    engaged.blobs.push(blob);
                    engaged.snapshot.mode = m;
                    Some(blob)
                }
                Err(e) => {
                    eprintln!("nightjar: kms: the display mode for {} fps cannot be made ({e}); it stays", self.wishes.fps);
                    None
                }
            },
            None => None,
        };
        let mut tried = Vec::new();
        let mut chosen = None;
        for candidate in &layouts {
            if candidate.background.is_some() && engaged.background.is_none() {
                match card.dumb_buffer(w, h, plan::XRGB8888) {
                    Ok(buffer) => engaged.background = Some(buffer),
                    Err(e) => {
                        tried.push(format!("picture on plane {} (no black background: {e})", candidate.video));
                        continue;
                    }
                }
            }
            engaged.layout = candidate.clone();
            engaged.overlay.plane = candidate.overlay.unwrap_or(0);
            // Tested with the overlay up, so showing it later fits the card's limits too.
            let tested = Self::engage_sets(card, &mut engaged, &self.output, &self.wishes, mode_blob, true)
                .and_then(|sets| card.commit(&sets, drm::ATOMIC_TEST_ONLY | drm::ATOMIC_ALLOW_MODESET).map_err(|e| e.to_string()));
            match tested {
                Ok(()) => {
                    chosen = Some(candidate.clone());
                    break;
                }
                Err(e) => tried.push(format!("picture on plane {} ({e})", candidate.video)),
            }
        }
        let Some(chosen) = chosen else {
            Self::release(card, engaged);
            return Err(format!("the card refused every layout: {}", tried.join(", ")));
        };
        engaged.overlay.plane = chosen.overlay.unwrap_or(0);
        engaged.layout = chosen;
        // The commit that takes the display with the first picture; the overlay plane stays down
        // until something shows. Only this commit's changes are put back at the end.
        engaged.restore.clear();
        let committed = Self::engage_sets(card, &mut engaged, &self.output, &self.wishes, mode_blob, false)
            .and_then(|sets| card.commit(&sets, drm::ATOMIC_ALLOW_MODESET).map_err(|e| format!("the card refused the stream's first commit: {e}")));
        if let Err(why) = committed {
            Self::release(card, engaged);
            return Err(why);
        }
        engaged.shown_fb = engaged.next_fb;
        engaged.shown_rect = Some(engaged.rect);
        engaged.props_dirty = false;
        // The mode goes back as a blob of its own: the one the CRTC had may be gone by then.
        if mode_blob.is_some() {
            let mode_prop = engaged.snapshot.crtc.get("MODE_ID").map(|(info, _)| info.id);
            match (mode_prop, card.mode_blob(&original_mode)) {
                (Some(prop), Ok(blob)) => {
                    engaged.blobs.push(blob);
                    for entry in engaged.restore.iter_mut().filter(|(o, p, _)| *o == self.output.crtc_id && *p == prop) {
                        entry.2 = u64::from(blob);
                    }
                }
                (_, Err(e)) => eprintln!("nightjar: kms: the display mode to go back to cannot be made ({e}); the old one is asked for"),
                (None, Ok(blob)) => card.destroy_blob(blob),
            }
        }
        eprintln!(
            "nightjar: kms: the stream shows on plane {}{}, the overlay on plane {}, {} {}; HDR {}, VRR {}",
            engaged.layout.video,
            engaged.layout.background.map_or(String::new(), |b| format!(" over a black plane {b}")),
            engaged.overlay.plane,
            plan::fourcc_name(engaged.format.0),
            if engaged.format.1 == plan::MOD_INVALID { "implicit".to_owned() } else { format!("modifier {:#x}", engaged.format.1) },
            if hdr.is_some() { "on" } else { "off" },
            match (self.wishes.vrr, engaged.vrr_on) {
                (false, _) => "off",
                (true, false) => "asked for, which this display does not take",
                (true, true) if plan::may_tear(self.wishes.tearing, true, self.wishes.tear_free) => "on, pictures at once (they can tear)",
                (true, true) => "on, whole pictures",
            },
        );
        self.engaged = Some(engaged);
        Ok(())
    }

    fn props_of(engaged: &Engaged, object: Object) -> Option<&Props> {
        match object {
            Object::Connector => Some(&engaged.snapshot.connector),
            Object::Crtc => Some(&engaged.snapshot.crtc),
            Object::Plane(id) => engaged.snapshot.planes.iter().find(|(p, _)| p.id == id).map(|(_, props)| props),
        }
    }

    fn object_id(output: &plan::Output, object: Object) -> u32 {
        match object {
            Object::Connector => output.connector_id,
            Object::Crtc => output.crtc_id,
            Object::Plane(id) => id,
        }
    }

    /// Appends `name` = `value` on `object` to `sets`, noting the value it had for the restore.
    /// A property the object lacks is left out.
    pub(super) fn put(engaged: &mut Engaged, output: &plan::Output, sets: &mut Vec<(u32, u32, u64)>, object: Object, name: &str, value: u64) {
        let Some(props) = Self::props_of(engaged, object) else { return };
        let Some((info, before)) = props.get(name).map(|(info, v)| (info.id, *v)) else { return };
        let id = Self::object_id(output, object);
        if !engaged.restore.iter().any(|(o, p, _)| *o == id && *p == info) {
            engaged.restore.push((id, info, before));
        }
        sets.retain(|(o, p, _)| !(*o == id && *p == info));
        sets.push((id, info, value));
    }

    pub(super) fn put_plane(engaged: &mut Engaged, output: &plan::Output, sets: &mut Vec<(u32, u32, u64)>, plane: u32, fb: u32, rect: Option<plan::PlaneRect>) {
        let object = Object::Plane(plane);
        match rect {
            Some(r) => {
                Self::put(engaged, output, sets, object, "FB_ID", u64::from(fb));
                Self::put(engaged, output, sets, object, "CRTC_ID", u64::from(output.crtc_id));
                Self::put(engaged, output, sets, object, "SRC_X", 0);
                Self::put(engaged, output, sets, object, "SRC_Y", 0);
                Self::put(engaged, output, sets, object, "SRC_W", u64::from(r.src_w) << 16);
                Self::put(engaged, output, sets, object, "SRC_H", u64::from(r.src_h) << 16);
                Self::put(engaged, output, sets, object, "CRTC_X", i64::from(r.crtc_x) as u64);
                Self::put(engaged, output, sets, object, "CRTC_Y", i64::from(r.crtc_y) as u64);
                Self::put(engaged, output, sets, object, "CRTC_W", u64::from(r.crtc_w));
                Self::put(engaged, output, sets, object, "CRTC_H", u64::from(r.crtc_h));
            }
            None => {
                Self::put(engaged, output, sets, object, "FB_ID", 0);
                Self::put(engaged, output, sets, object, "CRTC_ID", 0);
            }
        }
    }

    /// The stream's properties (`plan::stream_properties`) as sets, making the HDR blob.
    pub(super) fn stream_sets(card: &Card, engaged: &mut Engaged, output: &plan::Output, wishes: &Wishes, sets: &mut Vec<(u32, u32, u64)>) -> Result<(), String> {
        let caps = caps_of(&engaged.snapshot.connector, &engaged.snapshot.crtc);
        let color_props = engaged.snapshot.planes.iter().any(|(p, _)| p.id == engaged.layout.video && p.color_props);
        let stream = plan::Stream {
            ten_bit: engaged.format.0 == plan::P010,
            hdr: engaged.hdr,
            vrr: wishes.vrr,
            color_space: engaged.color.0,
            color_range: engaged.color.1,
        };
        for set in plan::stream_properties(&caps, &stream, engaged.layout.video, color_props) {
            let value = match &set.value {
                Value::Number(n) => *n,
                Value::Enum(name) => match Self::props_of(engaged, set.object).and_then(|p| p.enum_value(set.name, name)) {
                    Some(v) => v,
                    None => continue,
                },
                Value::HdrBlob(None) => 0,
                Value::HdrBlob(Some(bytes)) => {
                    let blob = card.create_blob(bytes).map_err(|e| format!("the HDR metadata cannot be made: {e}"))?;
                    engaged.blobs.push(blob);
                    u64::from(blob)
                }
            };
            Self::put(engaged, output, sets, set.object, set.name, value);
        }
        Ok(())
    }

    /// The engaging commit: mode, stream properties, zpos, the picture plane with the first
    /// picture, the black background, the overlay (`overlay_up` for the test, else down), and
    /// every other plane on the CRTC down.
    fn engage_sets(card: &Card, engaged: &mut Engaged, output: &plan::Output, wishes: &Wishes, mode_blob: Option<u32>, overlay_up: bool) -> Result<Vec<(u32, u32, u64)>, String> {
        let mut sets = Vec::new();
        if let Some(blob) = mode_blob {
            Self::put(engaged, output, &mut sets, Object::Crtc, "MODE_ID", u64::from(blob));
            Self::put(engaged, output, &mut sets, Object::Crtc, "ACTIVE", 1);
            Self::put(engaged, output, &mut sets, Object::Connector, "CRTC_ID", u64::from(output.crtc_id));
        }
        Self::stream_sets(card, engaged, output, wishes, &mut sets)?;
        for (plane, z) in engaged.layout.zpos.clone() {
            Self::put(engaged, output, &mut sets, Object::Plane(plane), "zpos", z);
        }
        let full = Self::full_rect(engaged);
        let (video, rect, fb) = (engaged.layout.video, engaged.rect, engaged.next_fb);
        Self::put_plane(engaged, output, &mut sets, video, fb, Some(rect));
        if let (Some(plane), Some(fb)) = (engaged.layout.background, engaged.background.as_ref().map(|b| b.fb)) {
            Self::put_plane(engaged, output, &mut sets, plane, fb, Some(full));
        }
        let overlay = engaged.overlay.plane;
        let overlay_fb = engaged.overlay.buffers[0].fb;
        Self::put_plane(engaged, output, &mut sets, overlay, overlay_fb, overlay_up.then_some(full));
        let ours = [Some(video), engaged.layout.background, Some(overlay)];
        let others: Vec<u32> = engaged
            .snapshot
            .planes
            .iter()
            .filter(|(p, _)| p.crtc_id == output.crtc_id && p.kind != plan::PlaneKind::Cursor && !ours.contains(&Some(p.id)))
            .map(|(p, _)| p.id)
            .collect();
        for plane in others {
            Self::put_plane(engaged, output, &mut sets, plane, 0, None);
        }
        Ok(sets)
    }

    /// The whole CRTC, for the background and the overlay.
    pub(super) fn full_rect(engaged: &Engaged) -> plan::PlaneRect {
        let (w, h) = (u32::from(engaged.snapshot.mode.hdisplay), u32::from(engaged.snapshot.mode.vdisplay));
        plan::PlaneRect { src_w: w, src_h: h, crtc_x: 0, crtc_y: 0, crtc_w: w, crtc_h: h }
    }
}
