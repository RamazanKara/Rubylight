//! `GET /usb/tunnel`: the byte pipe of one forwarded USB device (feature
//! `usb`). Defined for 3.0; not yet served.
//!
//! The client asks for a device on the `0x553A` control channel and gets a
//! token back in its `TUNNEL_READY` message. It then opens this path on the
//! mutual-TLS port with `Connection: Upgrade`, [`UPGRADE`] and the token in
//! [`TOKEN_HEADER`], and the host answers `101 Switching Protocols`. After
//! that the bytes are standard USB/IP: the client is the USB/IP server and the
//! host its client.

/// Path under [`crate::PREFIX`].
pub const TUNNEL_PATH: &str = "/usb/tunnel";
/// The `Upgrade` header value.
pub const UPGRADE: &str = "rubylight-usbip/1";
/// The header carrying the token, as 32 lowercase hex digits.
pub const TOKEN_HEADER: &str = "X-Rubylight-Usb-Token";
/// Bytes in a token before hex encoding.
pub const TOKEN_BYTES: usize = 16;
/// Seconds a token stays usable. A token works once, for the device whose
/// certificate asked for it.
pub const TOKEN_TTL_SECS: u64 = 30;
