//! Moonlight's HDR mastering metadata and the matching encoder values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub primaries: [[u16; 2]; 3],
    pub white: [u16; 2],
    pub maximum_nits: u16,
    /// Units of 1/10,000 nit, as in the Moonlight and HEVC metadata syntax.
    pub minimum: u16,
    pub max_cll: u16,
    pub max_fall: u16,
    pub full_frame_nits: u16,
}
impl Default for Metadata {
    fn default() -> Self {
        Self {
            primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white: [15635, 16450],
            maximum_nits: 1000,
            minimum: 1,
            max_cll: 0,
            max_fall: 0,
            full_frame_nits: 0,
        }
    }
}
impl Metadata {
    pub fn display(maximum: f32, minimum: f32, full_frame: f32) -> Self {
        let mut metadata = Self::default();
        if maximum.is_finite() && maximum > 0. {
            metadata.maximum_nits = maximum.clamp(1., u16::MAX as f32) as u16;
        }
        if minimum.is_finite() && minimum >= 0. {
            metadata.minimum = (minimum * 10000.).clamp(0., u16::MAX as f32) as u16;
        }
        if full_frame.is_finite() && full_frame > 0. {
            metadata.full_frame_nits = full_frame.clamp(1., metadata.maximum_nits as f32) as u16;
        }
        metadata
    }
    pub fn wire(self, enabled: bool) -> [u8; 27] {
        let mut bytes = [0; 27];
        bytes[0] = u8::from(enabled);
        let values = self
            .primaries
            .into_iter()
            .flatten()
            .chain(self.white)
            .chain([
                self.maximum_nits,
                self.minimum,
                self.max_cll,
                self.max_fall,
                self.full_frame_nits,
            ]);
        for (value, out) in values.zip(bytes[1..].as_chunks_mut::<2>().0) {
            *out = value.to_le_bytes();
        }
        bytes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_luminance_is_finite_bounded_and_uses_wire_units() {
        let metadata = Metadata::display(1499.9, 0.002, 800.);
        assert_eq!(metadata.maximum_nits, 1499);
        assert_eq!(metadata.minimum, 20);
        assert_eq!(metadata.full_frame_nits, 800);
        let wire = metadata.wire(true);
        assert_eq!(wire[0], 1);
        let fields: Vec<_> = wire[1..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes(*v))
            .collect();
        assert_eq!(
            fields,
            [
                35400, 14600, 8500, 39850, 6550, 2300, 15635, 16450, 1499, 20, 0, 0, 800
            ]
        );
        assert_eq!(
            Metadata::display(f32::NAN, f32::INFINITY, -1.),
            Metadata::default()
        );
        assert_eq!(Metadata::display(500., 0., 1000.).full_frame_nits, 500);
    }

    /// What `IDXGIOutput6::GetDesc1` reports for physical panels: an EDID
    /// without a luminance block gives zeros, which keep the 1000-nit default
    /// instead of advertising a 0-nit or 1-nit display to the client.
    #[test]
    fn physical_panel_luminance_reports_are_usable_or_default() {
        // HDR400 panel: 400 peak, 0.1 nit black, 400 sustained.
        let hdr400 = Metadata::display(400., 0.1, 400.);
        assert_eq!(
            (hdr400.maximum_nits, hdr400.minimum, hdr400.full_frame_nits),
            (400, 1000, 400)
        );
        // OLED: perfect black stays zero.
        assert_eq!(Metadata::display(1000., 0., 250.).minimum, 0);
        // No luminance block in the EDID: peak and full-frame unknown.
        let unknown = Metadata::display(0., 0., 0.);
        assert_eq!(unknown.maximum_nits, 1000);
        assert_eq!(unknown.full_frame_nits, 0);
        // Mini-LED claims past the 16-bit wire range are clamped, not wrapped.
        assert_eq!(
            Metadata::display(100_000., 0.0001, 5000.).maximum_nits,
            u16::MAX
        );
        // A full-frame figure above the peak is a driver error: cap it.
        assert_eq!(Metadata::display(600., 0.05, 900.).full_frame_nits, 600);
    }
}
