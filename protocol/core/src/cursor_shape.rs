//! Host cursor shapes (control messages 0x5540 and 0x5541), as Nightjar's
//! `docs/protocol/cursor-shape.md` version 1 defines them and its `cursor_shape.h` reads them.
//!
//! In a mode that draws the pointer on the client, the client sends a [`CursorShapeRequest`];
//! the host then leaves the cursor out of the video and sends a [`CursorShape`] whenever the
//! cursor's image or visibility changes. Both sides keep the [`CACHE_ENTRIES`] most recently
//! used ids. The client keeps sending absolute mouse positions; there is no position message.

use alloc::vec::Vec;

/// Control-stream message type of a [`CursorShapeRequest`] (client to host).
pub const CURSOR_SHAPE_REQUEST_MESSAGE_TYPE: u16 = 0x5540;
/// Control-stream message type of a [`CursorShape`] (host to client).
pub const CURSOR_SHAPE_MESSAGE_TYPE: u16 = 0x5541;
/// Encoded size of a [`CursorShapeRequest`].
pub const CURSOR_SHAPE_REQUEST_BYTES: usize = 4;
/// Size of every [`CursorShape`]'s header, and of a whole `Hide`, `Default` or `Show`.
pub const CURSOR_SHAPE_HEADER_BYTES: usize = 8;
/// Size of an `Image`'s fields between the header and the pixels.
pub const CURSOR_SHAPE_IMAGE_HEADER_BYTES: usize = 12;
/// Largest width or height.
pub const MAX_EDGE: u16 = 96;
/// Largest message: an `Image` of `MAX_EDGE` by `MAX_EDGE` pixels.
pub const MAX_PAYLOAD: usize = CURSOR_SHAPE_HEADER_BYTES
    + CURSOR_SHAPE_IMAGE_HEADER_BYTES
    + MAX_EDGE as usize * MAX_EDGE as usize * 4;
/// Ids each side keeps at least.
pub const CACHE_ENTRIES: usize = 32;
/// Pixel format of an `Image`: RGBA8888, straight alpha, sRGB.
pub const FORMAT_RGBA: u8 = 1;
/// Display scales an `Image` may carry, in percent.
pub const MIN_SCALE: u16 = 100;
pub const MAX_SCALE: u16 = 400;
/// Serverinfo element: the host sends shapes.
pub const SERVERINFO_FIELD: &str = "CursorShapeVersion";

const VERSION: u8 = 1;
const HIDE: u8 = 0;
const DEFAULT: u8 = 1;
const SHOW: u8 = 2;
const IMAGE: u8 = 3;

/// Client to host: start or stop shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorShapeRequest {
    /// Send shapes and leave the cursor out of the video; false stops both.
    pub enabled: bool,
    /// Largest width or height the client takes, 1-96.
    pub max_edge: u16,
}

impl CursorShapeRequest {
    pub fn encode(&self) -> [u8; CURSOR_SHAPE_REQUEST_BYTES] {
        let edge = self.max_edge.to_le_bytes();
        [VERSION, u8::from(self.enabled), edge[0], edge[1]]
    }

    /// Parses a request; version 0, an `enabled` other than 0 or 1 and a max edge outside
    /// 1-96 reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < CURSOR_SHAPE_REQUEST_BYTES || data[0] == 0 || data[1] > 1 {
            return None;
        }
        let max_edge = u16::from_le_bytes([data[2], data[3]]);
        (1..=MAX_EDGE).contains(&max_edge).then_some(Self {
            enabled: data[1] == 1,
            max_edge,
        })
    }
}

/// An `Image`'s picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    /// The host display's scale in percent, 100-400.
    pub scale: u16,
    /// `width * height` RGBA pixels, rows top to bottom.
    pub pixels: Vec<u8>,
}

impl CursorImage {
    fn valid(&self) -> bool {
        (1..=MAX_EDGE).contains(&self.width)
            && (1..=MAX_EDGE).contains(&self.height)
            && self.hotspot_x < self.width
            && self.hotspot_y < self.height
            && (MIN_SCALE..=MAX_SCALE).contains(&self.scale)
            && self.pixels.len() == usize::from(self.width) * usize::from(self.height) * 4
    }
}

/// Host to client: what the pointer looks like now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorShape {
    /// The host cursor is hidden.
    Hide,
    /// Draw the client's own default pointer.
    Default,
    /// Draw the image cached under `id`.
    Show { id: u32 },
    /// Cache this image under `id` and draw it.
    Image { id: u32, image: CursorImage },
}

impl CursorShape {
    /// The wire form, or `None` for id 0 on `Show` or `Image` or an image out of bounds.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let (kind, id) = match self {
            Self::Hide => (HIDE, 0),
            Self::Default => (DEFAULT, 0),
            Self::Show { id } => (SHOW, *id),
            Self::Image { id, .. } => (IMAGE, *id),
        };
        if matches!(kind, SHOW | IMAGE) && id == 0 {
            return None;
        }
        let mut out = Vec::with_capacity(CURSOR_SHAPE_HEADER_BYTES);
        out.extend_from_slice(&[VERSION, kind, 0, 0]);
        out.extend_from_slice(&id.to_le_bytes());
        if let Self::Image { image, .. } = self {
            if !image.valid() {
                return None;
            }
            for field in [image.width, image.height, image.hotspot_x, image.hotspot_y] {
                out.extend_from_slice(&field.to_le_bytes());
            }
            out.extend_from_slice(&[FORMAT_RGBA, 0]);
            out.extend_from_slice(&image.scale.to_le_bytes());
            out.extend_from_slice(&image.pixels);
        }
        Some(out)
    }

    /// Parses a shape. Version 0, an unknown kind, a non-zero id on `Hide` or `Default`, id 0
    /// on `Show` or `Image`, and an image out of bounds, in another format or of any length
    /// but 20 + width * height * 4 reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < CURSOR_SHAPE_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        match data[1] {
            HIDE if id == 0 => Some(Self::Hide),
            DEFAULT if id == 0 => Some(Self::Default),
            SHOW if id != 0 => Some(Self::Show { id }),
            IMAGE if id != 0 => {
                let header = CURSOR_SHAPE_HEADER_BYTES + CURSOR_SHAPE_IMAGE_HEADER_BYTES;
                if data.len() < header || data[16] != FORMAT_RGBA {
                    return None;
                }
                let image = CursorImage {
                    width: u16_at(8),
                    height: u16_at(10),
                    hotspot_x: u16_at(12),
                    hotspot_y: u16_at(14),
                    scale: u16_at(18),
                    pixels: data[header..].to_vec(),
                };
                image.valid().then_some(Self::Image { id, image })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn image() -> CursorImage {
        CursorImage {
            width: 2,
            height: 3,
            hotspot_x: 1,
            hotspot_y: 2,
            scale: 150,
            pixels: (0..24).collect(),
        }
    }

    #[test]
    fn constants_match_the_client_header() {
        let header = include_str!("../../../app/src/main/jni/moonlight-core/cursor_shape.h");
        for (name, value) in [
            (
                "REQUEST_TYPE",
                format!("{CURSOR_SHAPE_REQUEST_MESSAGE_TYPE:#06x}"),
            ),
            ("MESSAGE_TYPE", format!("{CURSOR_SHAPE_MESSAGE_TYPE:#06x}")),
            ("VERSION", format!("{VERSION}")),
            ("REQUEST_BYTES", format!("{CURSOR_SHAPE_REQUEST_BYTES}")),
            ("MAX_EDGE", format!("{MAX_EDGE}")),
            ("HEADER_BYTES", format!("{CURSOR_SHAPE_HEADER_BYTES}")),
            (
                "IMAGE_HEADER_BYTES",
                format!("{CURSOR_SHAPE_IMAGE_HEADER_BYTES}"),
            ),
        ] {
            let line = format!("#define RL_CURSOR_SHAPE_{name} {value}\n");
            assert!(header.replace("\r\n", "\n").contains(&line), "{line}");
        }
        assert!(
            header.contains("(RL_CURSOR_SHAPE_HEADER_BYTES + RL_CURSOR_SHAPE_IMAGE_HEADER_BYTES")
        );
        assert!(header.contains("RL_CURSOR_SHAPE_MAX_EDGE * RL_CURSOR_SHAPE_MAX_EDGE * 4)"));
        // The largest message the document names.
        assert_eq!(MAX_PAYLOAD, 36_884);
    }

    #[test]
    fn requests_round_trip() {
        let request = CursorShapeRequest {
            enabled: true,
            max_edge: 96,
        };
        assert_eq!(request.encode(), [1, 1, 96, 0]);
        assert_eq!(CursorShapeRequest::decode(&request.encode()), Some(request));
        let stop = CursorShapeRequest {
            enabled: false,
            max_edge: 1,
        };
        assert_eq!(CursorShapeRequest::decode(&stop.encode()), Some(stop));
        for data in [[1, 1, 0, 0], [1, 1, 97, 0], [1, 2, 96, 0], [0, 1, 96, 0]] {
            assert_eq!(CursorShapeRequest::decode(&data), None, "{data:?}");
        }
        assert_eq!(CursorShapeRequest::decode(&[1, 1, 96]), None);
    }

    #[test]
    fn shapes_match_the_documented_bytes() {
        assert_eq!(
            CursorShape::Hide.encode().unwrap(),
            [1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            CursorShape::Default.encode().unwrap(),
            [1, 1, 0, 0, 0, 0, 0, 0]
        );
        let show = CursorShape::Show { id: 0x0102_0304 };
        assert_eq!(show.encode().unwrap(), [1, 2, 0, 0, 4, 3, 2, 1]);
        let shape = CursorShape::Image {
            id: 7,
            image: image(),
        };
        let wire = shape.encode().unwrap();
        assert_eq!(wire.len(), 20 + 2 * 3 * 4);
        assert_eq!(wire[..8], [1, 3, 0, 0, 7, 0, 0, 0]);
        assert_eq!(wire[8..20], [2, 0, 3, 0, 1, 0, 2, 0, 1, 0, 150, 0]);
        assert_eq!(wire[20..], image().pixels[..]);
        for shape in [CursorShape::Hide, CursorShape::Default, show, shape] {
            assert_eq!(CursorShape::decode(&shape.encode().unwrap()), Some(shape));
        }
        let largest = CursorShape::Image {
            id: 1,
            image: CursorImage {
                width: 96,
                height: 96,
                hotspot_x: 95,
                hotspot_y: 95,
                scale: 400,
                pixels: alloc::vec![0xFF; 96 * 96 * 4],
            },
        };
        assert_eq!(largest.encode().unwrap().len(), MAX_PAYLOAD);
    }

    #[test]
    fn bad_shapes_are_refused() {
        assert_eq!(CursorShape::Show { id: 0 }.encode(), None);
        let with = |change: fn(&mut CursorImage)| {
            let mut image = image();
            change(&mut image);
            CursorShape::Image { id: 1, image }
        };
        for shape in [
            with(|i| i.width = 0),
            with(|i| i.height = 97),
            with(|i| i.hotspot_x = 2),
            with(|i| i.hotspot_y = 3),
            with(|i| i.scale = 99),
            with(|i| i.scale = 401),
            with(|i| {
                i.pixels.pop();
            }),
        ] {
            assert_eq!(shape.encode(), None, "{shape:?}");
        }
        assert_eq!(
            CursorShape::Image {
                id: 0,
                image: image()
            }
            .encode(),
            None
        );

        assert_eq!(
            CursorShape::decode(&[1, 0, 0, 0, 1, 0, 0, 0]),
            None,
            "HIDE with an id"
        );
        assert_eq!(
            CursorShape::decode(&[1, 1, 0, 0, 1, 0, 0, 0]),
            None,
            "DEFAULT with an id"
        );
        assert_eq!(
            CursorShape::decode(&[1, 2, 0, 0, 0, 0, 0, 0]),
            None,
            "SHOW without one"
        );
        assert_eq!(
            CursorShape::decode(&[1, 4, 0, 0, 1, 0, 0, 0]),
            None,
            "unknown kind"
        );
        assert_eq!(
            CursorShape::decode(&[0, 0, 0, 0, 0, 0, 0, 0]),
            None,
            "version 0"
        );
        assert_eq!(CursorShape::decode(&[1, 0, 0, 0, 0, 0, 0]), None, "short");
        let wire = CursorShape::Image {
            id: 7,
            image: image(),
        }
        .encode()
        .unwrap();
        let mut longer = wire.clone();
        longer.push(0);
        assert_eq!(CursorShape::decode(&longer), None, "one byte too many");
        assert_eq!(
            CursorShape::decode(&wire[..wire.len() - 1]),
            None,
            "one byte short"
        );
        let mut format = wire.clone();
        format[16] = 2;
        assert_eq!(CursorShape::decode(&format), None, "not RGBA");
        assert_eq!(CursorShape::decode(&wire[..19]), None, "header cut short");
    }
}
