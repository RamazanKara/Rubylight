//! Which pages may open the signalling socket on `webrtc_port`, which is plain `ws://` and needs no
//! sign-in to ask for pairing. A browser lets any page it shows open a WebSocket to any address and
//! read the replies, so without this a site on the internet could reach the host through the
//! browser of someone on the local network (cross-site WebSocket hijacking).
//!
//! What the clients send, from their own documentation (OpenstreamCo's protocol and READMEs):
//! the Tizen TV app is a packaged widget (`.wgt`), whose requests carry `Origin: file://`; the
//! protocol's browser client is served from `https://openstreamco.github.io` or self-hosted at
//! `http://localhost:8770`; native programs and the project's own test client send no `Origin`.
//! Nightjar does not use this port. The console's Stream page and the play page signal over the
//! console's TLS ports instead, with checks of their own.

use butterpollo_stream::network::local_network;

/// The protocol's browser client on GitHub Pages: it opens `ws://` to the PC with the browser's
/// local network permission.
const BROWSER_CLIENT: &str = "https://openstreamco.github.io";

/// Names that only resolve inside a home network: they can't be a site on the internet.
const LOCAL_SUFFIXES: &[&str] = &[".local", ".lan", ".home.arpa", ".internal", ".localdomain"];

/// What the `Origin` header says about who opens the socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Opener {
    /// No `Origin`, or a packaged app's (`file://` and other schemes that aren't the web's): a TV
    /// app or a program, which may learn the Wake-on-LAN address before signing in.
    App,
    /// A web page the host trusts: on this PC or the local network, the protocol's browser client,
    /// or an origin listed in `csrf_allowed_origins`. It learns the Wake-on-LAN address only after
    /// signing in.
    Page,
    /// Any other page, or `null`, which a sandboxed frame of any site sends. Refused.
    Refused,
}

/// Whether a page's host is on this PC or the local network: an address there, `localhost`, a
/// name without a dot, a name under a suffix that only resolves at home, or the PC's own name.
fn local_host(host: &str, pc_name: Option<&str>) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return local_network(address);
    }
    !host.is_empty()
        && (host == "localhost"
            || host.ends_with(".localhost")
            || !host.contains('.')
            || LOCAL_SUFFIXES.iter().any(|suffix| host.ends_with(suffix))
            || pc_name.is_some_and(|name| host == name.to_ascii_lowercase()))
}

/// Who opens the socket, from its `Origin` headers (none, or every one of them) and the
/// `csrf_allowed_origins` setting.
pub(super) fn opener<'a>(
    origins: impl IntoIterator<Item = &'a str>,
    allowed: &[String],
    pc_name: Option<&str>,
) -> Opener {
    let mut opener = Opener::App;
    for origin in origins {
        let this = one(origin.trim(), allowed, pc_name);
        if this == Opener::Refused {
            return this;
        }
        if this == Opener::Page {
            opener = Opener::Page;
        }
    }
    opener
}

fn one(origin: &str, allowed: &[String], pc_name: Option<&str>) -> Opener {
    if allowed.iter().any(|entry| entry == origin) || origin == BROWSER_CLIENT {
        return Opener::Page;
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        // `null`, or no origin at all.
        return Opener::Refused;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Opener::Refused;
    }
    if !matches!(scheme.as_str(), "http" | "https") {
        return Opener::App;
    }
    // An origin is scheme, host and port: no path, user or query.
    if rest.contains(['/', '@', '?', '#']) {
        return Opener::Refused;
    }
    let host = match rest.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') && port.bytes().all(|b| b.is_ascii_digit()) => {
            host
        }
        _ => rest,
    };
    if local_host(host, pc_name) {
        Opener::Page
    } else {
        Opener::Refused
    }
}

/// What a refused page is told, and what the host log says.
pub(super) fn refusal(origin: &str) -> String {
    format!(
        "The WebRTC server refused a page from {origin}: only pages on this PC or the local network, and the WebRTC browser client, may use it. To allow this page, add its address to Trusted origins (csrf_allowed_origins)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judged(origin: Option<&str>) -> Opener {
        opener(
            origin,
            &["https://games.example.com".into()],
            Some("GAMING-PC"),
        )
    }

    #[test]
    fn apps_and_programs_open_the_socket_as_apps() {
        for origin in [
            None,
            Some("file://"),
            Some("app://tizen"),
            Some("tizen://x"),
        ] {
            assert_eq!(judged(origin), Opener::App, "{origin:?}");
        }
    }

    #[test]
    fn local_pages_the_browser_client_and_trusted_origins_are_pages() {
        for origin in [
            "http://localhost:8770",
            "http://127.0.0.1:8770",
            "http://192.168.4.20:8080",
            "http://[fd00::20]:8080",
            "http://[::1]",
            "https://openstreamco.github.io",
            "https://games.example.com",
            "http://gaming-pc:8080",
            "http://nas",
            "http://nas.local",
            "http://tv.home.arpa",
            "https://100.101.102.103",
        ] {
            assert_eq!(judged(Some(origin)), Opener::Page, "{origin}");
        }
    }

    #[test]
    fn internet_pages_sandboxed_frames_and_odd_origins_are_refused() {
        for origin in [
            "https://evil.example",
            "http://evil.example:8000",
            "http://203.0.113.9",
            "http://[2001:db8::1]",
            "null",
            "",
            "https://openstreamco.github.io.evil.example",
            "https://evil.github.io",
            "http://192.168.4.20.evil.example",
            "http://localhost.evil.example",
            "http://user@localhost",
            "http://localhost/path",
            "://localhost",
        ] {
            assert_eq!(judged(Some(origin)), Opener::Refused, "{origin:?}");
        }
        // One refused origin among several refuses the request.
        assert_eq!(
            opener(["http://localhost", "https://evil.example"], &[], None),
            Opener::Refused
        );
    }

    #[test]
    fn only_an_app_on_the_plain_port_learns_the_wake_address_before_signing_in() {
        use super::super::{Opened, connection::wake_address};
        let lookup = || Some("AA:BB:CC:DD:EE:FF".to_owned());
        assert_eq!(
            wake_address(Opened::PlainApp, lookup).as_deref(),
            Some("AA:BB:CC:DD:EE:FF")
        );
        for opened in [Opened::PlainPage, Opened::Tls] {
            assert_eq!(
                wake_address(opened, || panic!("looked up for {opened:?}")),
                None
            );
        }
    }

    /// The status line of an upgrade on `webrtc_port` with `origin`, and the first frame after it.
    async fn upgrade(address: std::net::SocketAddr, origin: Option<&str>) -> (String, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        let request = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
        }
        let header = String::from_utf8_lossy(&header).into_owned();
        let status = header.lines().next().unwrap_or("").to_owned();
        if !status.contains(" 101 ") {
            let mut body = Vec::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                stream.read_to_end(&mut body),
            )
            .await;
            return (status, String::from_utf8_lossy(&body).into_owned());
        }
        let mut frame = [0u8; 2];
        stream.read_exact(&mut frame).await.unwrap();
        let length = match frame[1] & 0x7f {
            126 => {
                let mut extended = [0u8; 2];
                stream.read_exact(&mut extended).await.unwrap();
                usize::from(u16::from_be_bytes(extended))
            }
            n => usize::from(n),
        };
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload).await.unwrap();
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    #[tokio::test]
    async fn an_internet_page_is_refused_before_the_upgrade_and_apps_and_local_pages_are_not() {
        let f = butterpollo_stream::state::test_support::Fixture::new();
        let _running = super::super::start_with_cap(
            f.host.clone(),
            [127, 0, 0, 1].into(),
            0,
            0,
            None,
            super::super::Limits::default(),
            8,
        )
        .unwrap();
        let address = crate::WebRtcRuntime::for_host(&f.host)
            .clients
            .status()
            .signalling
            .unwrap();
        for origin in ["https://evil.example", "null", "http://203.0.113.9:8000"] {
            let (status, body) = upgrade(address, Some(origin)).await;
            assert!(status.contains(" 403 "), "{origin}: {status}");
            assert!(body.contains("csrf_allowed_origins"), "{origin}: {body}");
        }
        for origin in [
            None,
            Some("file://"),
            Some("http://localhost:8770"),
            Some(BROWSER_CLIENT),
        ] {
            let (status, frame) = upgrade(address, origin).await;
            assert!(status.contains(" 101 "), "{origin:?}: {status}");
            assert!(frame.contains("auth-required"), "{origin:?}: {frame}");
        }
    }
}
