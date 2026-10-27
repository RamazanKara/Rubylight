//! The USB tunnel (`GET /api/native/v1/usb/tunnel`, feature `usb`): the connection one
//! forwarded device's USB/IP runs over, opened with the token of a `TunnelReady`. Gate it on
//! `Feature::UsbForwarding`; `crate::usb` serves the device over it.

use super::native::error_of;
use super::native_api::PREFIX;
use super::transport::{self, Request, Timeouts};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};
use rubylight_protocol::usb::TOKEN_BYTES;

pub use super::transport::Upgraded as UsbTunnel;

/// The tunnel's path under the native API prefix, its `Upgrade` value and its token header.
/// The pinned `rubylight-native-api` predates them; the host's interop tests hold them to the
/// host's own.
pub const TUNNEL_PATH: &str = "/usb/tunnel";
pub const UPGRADE: &str = "rubylight-usbip/1";
pub const TOKEN_HEADER: &str = "X-Rubylight-Usb-Token";

impl HostClient {
    /// Opens the USB tunnel for the token of a `TunnelReady`. A refusal is an
    /// [`Error::Api`]: `usb_disabled`, `usb_limit` (a used token) or `not_found` (an unknown
    /// or expired one).
    pub fn open_usb_tunnel(&self, token: &[u8; TOKEN_BYTES]) -> Result<UsbTunnel, Error> {
        let path = format!("{}{TUNNEL_PATH}", PREFIX.trim_start_matches('/'));
        let target = self.url_target(&path, None);
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target: &target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(READ_TIMEOUT),
            },
            body_limit: 0,
        };
        let token = super::der::hex_encode(token).to_ascii_lowercase();
        match transport::upgrade(&request, &self.cancel, UPGRADE, &[(TOKEN_HEADER, &token)])? {
            Ok(tunnel) => Ok(tunnel),
            Err(response) => Err(error_of(response, &path)),
        }
    }
}
