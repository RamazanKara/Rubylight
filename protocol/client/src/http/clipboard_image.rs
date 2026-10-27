//! The image clipboard: `type=png` on `/actions/clipboard`, beside the text form and with its
//! permissions (`docs/native-api.md`, "Image clipboard"). Gate it on `Feature::ImageClipboard`.
//!
//! Only PNGs travel. Before one is sent, and after one arrives, its signature and header are
//! checked against the host's limits, so an image the host would refuse fails here, by name,
//! before a byte moves, and a body that is no PNG never reaches the device's clipboard. The
//! route answers with the status alone; each one the host documents has its own error.

use super::transport::{self, Request, Timeouts, Upload};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT};
use std::time::Duration;

/// The most bytes an image may have: the top of the host's `clipboard_image_max_mb` (1 to 64,
/// 16 by default). A host set lower answers 413, [`ImageClipboardError::TooLarge`].
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// The widest and tallest image the host decodes.
pub const MAX_IMAGE_SIDE: u32 = 16384;
/// The most pixels the host decodes: 32 megapixels.
pub const MAX_IMAGE_PIXELS: u64 = 1 << 25;
pub const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// 64 MiB over a slow link takes a while; the host answers within this once it has the body.
const IMAGE_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why the image clipboard gave no image or took none.
#[derive(Debug)]
pub enum ImageClipboardError {
    /// Over the byte or pixel limits: found here, or the host's 413 (its own byte limit may be
    /// lower than [`MAX_CLIPBOARD_IMAGE_BYTES`]).
    TooLarge(String),
    /// Not a PNG, or a PNG whose header is broken: found here, or the host's 400.
    NotPng(String),
    /// 415: the host can't convert the image on its clipboard, or decode the one sent.
    Unsupported,
    /// 403: the image clipboard is off on the host, the device lacks the clipboard permission,
    /// or it has no stream of its own (or only a background one).
    Forbidden,
    Host(Error),
}

impl std::fmt::Display for ImageClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageClipboardError::TooLarge(why) => write!(f, "the image is too large: {why}"),
            ImageClipboardError::NotPng(why) => write!(f, "not a PNG image: {why}"),
            ImageClipboardError::Unsupported => f.write_str("the PC can't convert this image"),
            ImageClipboardError::Forbidden => {
                f.write_str("the PC refused its clipboard to this device")
            }
            ImageClipboardError::Host(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ImageClipboardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ImageClipboardError::Host(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for ImageClipboardError {
    fn from(e: Error) -> Self {
        ImageClipboardError::Host(e)
    }
}

/// The width and height a PNG's header gives, once its signature, its first chunk (IHDR, 13
/// bytes) and the host's limits check out. Nothing is decoded.
pub fn png_size(png: &[u8]) -> Result<(u32, u32), ImageClipboardError> {
    if png.len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Err(ImageClipboardError::TooLarge(format!(
            "{} bytes, over {MAX_CLIPBOARD_IMAGE_BYTES}",
            png.len()
        )));
    }
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(ImageClipboardError::NotPng("no PNG signature".into()));
    }
    let header = png
        .get(8..24)
        .filter(|chunk| chunk[..8] == *b"\0\0\0\x0dIHDR")
        .ok_or_else(|| ImageClipboardError::NotPng("no IHDR header".into()))?;
    let width = u32::from_be_bytes(header[8..12].try_into().unwrap());
    let height = u32::from_be_bytes(header[12..16].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(ImageClipboardError::NotPng(format!(
            "an empty {width}x{height} image"
        )));
    }
    if width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(ImageClipboardError::TooLarge(format!(
            "{width}x{height}, over {MAX_IMAGE_SIDE} a side or {MAX_IMAGE_PIXELS} pixels"
        )));
    }
    Ok((width, height))
}

/// The error a non-2xx status of the route stands for.
fn refused(status: u16, reason: String) -> ImageClipboardError {
    match status {
        400 => ImageClipboardError::NotPng("the PC could not read it".into()),
        403 => ImageClipboardError::Forbidden,
        413 => ImageClipboardError::TooLarge("over the PC's limit".into()),
        415 => ImageClipboardError::Unsupported,
        code => ImageClipboardError::Host(Error::Status {
            code: i32::from(code),
            message: reason,
        }),
    }
}

/// A GET's answer: `None` for 404 (no image on the PC's clipboard), else a checked PNG.
fn read_answer(response: transport::Response) -> Result<Option<Vec<u8>>, ImageClipboardError> {
    match response.status {
        404 => return Ok(None),
        200 => {}
        status => return Err(refused(status, response.reason)),
    }
    let png = response.content_type.as_deref().is_some_and(|mime| {
        mime.split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("image/png")
    });
    if !png {
        return Err(Error::Malformed(format!(
            "the PC sent {:?} for a clipboard image",
            response.content_type.unwrap_or_default()
        ))
        .into());
    }
    png_size(&response.body)?;
    Ok(Some(response.body))
}

/// A POST's answer: 200 with an empty body.
fn write_answer(response: transport::Response) -> Result<(), ImageClipboardError> {
    match response.status {
        200 if response.body.is_empty() => Ok(()),
        200 => Err(Error::Malformed("the PC did not accept the clipboard image".into()).into()),
        status => Err(refused(status, response.reason)),
    }
}

impl HostClient {
    fn clipboard_image_request(&self, png: Option<&[u8]>) -> Result<transport::Response, Error> {
        let target = self.url_target("actions/clipboard", Some("type=png"));
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target: &target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(IMAGE_READ_TIMEOUT),
            },
            body_limit: MAX_CLIPBOARD_IMAGE_BYTES,
        };
        let Some(png) = png else {
            return transport::get(&request, &self.cancel);
        };
        // Streamed like a file: a host that refuses the image (413, 403) before reading it
        // closes the connection, and its answer, not the failed write, is what comes back.
        let upload = Upload {
            reader: &mut &png[..],
            length: png.len() as u64,
            content_type: "image/png",
            method: "POST",
        };
        transport::upload(
            &request,
            &self.cancel,
            upload,
            &mut |_, _| {},
            &Default::default(),
        )
    }

    /// The image on the PC's clipboard as a PNG, or `None` while it holds no image.
    pub fn clipboard_image(&self) -> Result<Option<Vec<u8>>, ImageClipboardError> {
        read_answer(self.clipboard_image_request(None)?)
    }

    /// Puts `png` on the PC's clipboard. It is checked against the host's limits first.
    pub fn send_clipboard_image(&self, png: &[u8]) -> Result<(), ImageClipboardError> {
        png_size(png)?;
        write_answer(self.clipboard_image_request(Some(png))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG's signature and IHDR chunk; the rest isn't read here.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(b"\0\0\0\x0dIHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png
    }

    fn response(status: u16, content_type: Option<&str>, body: Vec<u8>) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: content_type.map(str::to_owned),
            body,
        }
    }

    #[test]
    fn a_png_header_is_checked_against_the_hosts_limits() {
        assert_eq!(png_size(&png_header(2, 1)).unwrap(), (2, 1));
        assert_eq!(png_size(&png_header(16384, 2048)).unwrap(), (16384, 2048));
        for (width, height) in [(16385, 1), (1, 16385), (8193, 4096)] {
            assert!(
                matches!(
                    png_size(&png_header(width, height)),
                    Err(ImageClipboardError::TooLarge(_))
                ),
                "{width}x{height}"
            );
        }
        let mut large = png_header(1, 1);
        large.resize(MAX_CLIPBOARD_IMAGE_BYTES + 1, 0);
        assert!(matches!(
            png_size(&large),
            Err(ImageClipboardError::TooLarge(_))
        ));
        let mut wrong_chunk = png_header(1, 1);
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        for broken in [
            b"GIF89a".to_vec(),
            Vec::new(),
            PNG_SIGNATURE.to_vec(),
            png_header(1, 1)[..20].to_vec(),
            wrong_chunk,
            png_header(0, 1),
            png_header(1, 0),
        ] {
            assert!(
                matches!(png_size(&broken), Err(ImageClipboardError::NotPng(_))),
                "{broken:?}"
            );
        }
    }

    #[test]
    fn every_status_the_route_documents_has_its_error() {
        let png = png_header(3, 3);
        assert_eq!(
            read_answer(response(200, Some("image/png"), png.clone())).unwrap(),
            Some(png.clone())
        );
        assert_eq!(
            read_answer(response(200, Some("Image/PNG; x=1"), png.clone())).unwrap(),
            Some(png.clone())
        );
        assert!(read_answer(response(404, None, Vec::new()))
            .unwrap()
            .is_none());
        // An answer of the wrong type, or no PNG, never reaches the device's clipboard.
        for (content_type, body) in [
            (Some("text/plain"), png.clone()),
            (None, png.clone()),
            (Some("image/png"), b"<root status_code=\"404\"/>".to_vec()),
            (Some("image/png"), png_header(20000, 1)),
        ] {
            assert!(
                read_answer(response(200, content_type, body)).is_err(),
                "{content_type:?}"
            );
        }
        for (status, check) in [
            (
                400,
                (|e| matches!(e, ImageClipboardError::NotPng(_)))
                    as fn(&ImageClipboardError) -> bool,
            ),
            (403, |e| matches!(e, ImageClipboardError::Forbidden)),
            (413, |e| matches!(e, ImageClipboardError::TooLarge(_))),
            (415, |e| matches!(e, ImageClipboardError::Unsupported)),
            (500, |e| {
                matches!(
                    e,
                    ImageClipboardError::Host(Error::Status { code: 500, .. })
                )
            }),
        ] {
            let read = read_answer(response(status, None, Vec::new())).unwrap_err();
            let write = write_answer(response(status, None, Vec::new())).unwrap_err();
            assert!(
                check(&read) && check(&write),
                "{status}: {read:?} {write:?}"
            );
        }
        assert!(write_answer(response(200, None, Vec::new())).is_ok());
        // A 404 on a write is the route missing, not an empty clipboard.
        assert!(matches!(
            write_answer(response(404, None, Vec::new())),
            Err(ImageClipboardError::Host(Error::Status { code: 404, .. }))
        ));
        assert!(matches!(
            write_answer(response(200, Some("application/xml"), b"<root/>".to_vec())),
            Err(ImageClipboardError::Host(Error::Malformed(_)))
        ));
    }
}
