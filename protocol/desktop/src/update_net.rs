//! The HTTP under the update check and its download: one GET that never follows a redirect on its
//! own, so that self_update.rs can judge every address before anything connects to it. WinHTTP on
//! Windows (the system's proxy settings and certificate store, nothing bundled), ureq elsewhere.

use std::io::Read;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// With its query.
    pub path: String,
}

impl Url {
    /// An http or https address with a plain host name or IPv4 address: no user part, no IPv6, no
    /// spaces or control characters, so what is judged here is what gets connected to.
    pub fn parse(text: &str) -> Option<Self> {
        let (secure, rest) = match (text.strip_prefix("https://"), text.strip_prefix("http://")) {
            (Some(rest), _) => (true, rest),
            (_, Some(rest)) => (false, rest),
            _ => return None,
        };
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(at) => rest.split_at(at),
            None => (rest, "/"),
        };
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, if secure { 443 } else { 80 }),
        };
        let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'-';
        if host.is_empty() || !host.bytes().all(plain) || !path.starts_with('/') || path.bytes().any(|b| b <= b' ' || b == 0x7F) {
            return None;
        }
        Some(Self { secure, host: host.to_ascii_lowercase(), port, path: path.to_owned() })
    }

    /// Where a redirect's `Location` points from here.
    pub fn resolve(&self, location: &str) -> Option<Self> {
        if location.starts_with('/') && !location.starts_with("//") {
            if location.bytes().any(|b| b <= b' ' || b == 0x7F) { return None; }
            return Some(Self { path: location.to_owned(), ..self.clone() });
        }
        Self::parse(location)
    }
}

pub struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// One GET. `api` asks for GitHub's JSON. Redirects come back as they are.
pub trait Transport: Send + Sync {
    fn get(&self, url: &Url, api: bool) -> Result<Response, String>;
}

pub struct Native {
    pub agent: String,
}

#[cfg(windows)]
impl Transport for Native {
    fn get(&self, url: &Url, api: bool) -> Result<Response, String> {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Networking::WinHttp::*;

        fn open(handle: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
            if handle.is_null() { Err(format!("{what}: {}", windows::core::Error::from_thread())) } else { Ok(Handle(handle)) }
        }

        // SAFETY: WinHTTP calls on handles owned here; strings and buffers outlive the calls.
        unsafe {
            let session = open(WinHttpOpen(&HSTRING::from(self.agent.as_str()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0), "WinHttpOpen")?;
            WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000).map_err(|e| format!("timeouts: {e}"))?;
            let connection = open(WinHttpConnect(session.0, &HSTRING::from(url.host.as_str()), url.port, 0), "connect")?;
            let flags = if url.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
            let request = open(WinHttpOpenRequest(connection.0, &HSTRING::from("GET"), &HSTRING::from(url.path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags), "request")?;
            let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
            WinHttpSetOption(Some(request.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never)).map_err(|e| format!("redirect policy: {e}"))?;
            let headers: Vec<u16> = if api { "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n" } else { "Accept: application/octet-stream\r\n" }.encode_utf16().collect();
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(|e| format!("send: {e}"))?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("response: {e}"))?;
            let number = |query: u32| -> Option<u32> {
                let mut value = 0u32;
                let mut size = std::mem::size_of::<u32>() as u32;
                WinHttpQueryHeaders(request.0, query | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some((&mut value as *mut u32).cast()), &mut size, std::ptr::null_mut()).ok()?;
                Some(value)
            };
            let status = number(WINHTTP_QUERY_STATUS_CODE).ok_or("no status")?;
            let length = number(WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from);
            // Release storage addresses are long, signed URLs.
            let mut text = vec![0u16; 8 * 1024];
            let mut size = (text.len() * 2) as u32;
            let location = WinHttpQueryHeaders(request.0, WINHTTP_QUERY_LOCATION, PCWSTR::null(), Some(text.as_mut_ptr().cast()), &mut size, std::ptr::null_mut())
                .ok()
                .map(|()| String::from_utf16_lossy(&text[..size as usize / 2]));
            Ok(Response { status: status as u16, location, length, body: Box::new(Body { request, _connection: connection, _session: session }) })
        }
    }
}

#[cfg(windows)]
struct Handle(*mut core::ffi::c_void);

// SAFETY: WinHTTP handles may be used from any thread; each of these is used from one at a time.
#[cfg(windows)]
unsafe impl Send for Handle {}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP returned, closed once.
        unsafe { let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0); }
    }
}

/// An open response. The fields close in this order: request, connection, session.
#[cfg(windows)]
struct Body {
    request: Handle,
    _connection: Handle,
    _session: Handle,
}

#[cfg(windows)]
impl Read for Body {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0u32;
        let room = buffer.len().min(u32::MAX as usize) as u32;
        // SAFETY: the request is open and the buffer holds `room` bytes.
        unsafe { windows::Win32::Networking::WinHttp::WinHttpReadData(self.request.0, buffer.as_mut_ptr().cast(), room, &mut read) }.map_err(std::io::Error::other)?;
        Ok(read as usize)
    }
}

#[cfg(not(windows))]
impl Transport for Native {
    fn get(&self, url: &Url, api: bool) -> Result<Response, String> {
        use std::time::Duration;

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent(self.agent.as_str())
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            .build()
            .into();
        let address = format!("{}://{}:{}{}", if url.secure { "https" } else { "http" }, url.host, url.port, url.path);
        let mut request = agent.get(&address);
        if api {
            request = request.header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        } else {
            request = request.header("Accept", "application/octet-stream");
        }
        let response = request.call().map_err(|e| e.to_string())?;
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (status, location, length) = (response.status().as_u16(), header("location"), header("content-length").and_then(|l| l.parse().ok()));
        Ok(Response { status, location, length, body: Box::new(response.into_body().into_reader()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_strictly() {
        let url = Url::parse("https://GitHub.com/a/b?c=d").unwrap();
        assert_eq!(url, Url { secure: true, host: "github.com".into(), port: 443, path: "/a/b?c=d".into() });
        assert_eq!(Url::parse("http://127.0.0.1:81").unwrap(), Url { secure: false, host: "127.0.0.1".into(), port: 81, path: "/".into() });
        for bad in ["ftp://github.com/", "https://", "https://user@github.com/", "https://github.com@evil.example/", "https://[::1]/", "https://github.com:x/", "https://git hub.com/", "https://github.com/a b", "github.com/a"] {
            assert_eq!(Url::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_redirect_resolves_from_here() {
        let from = Url::parse("https://github.com/a").unwrap();
        assert_eq!(from.resolve("/b?c").unwrap().path, "/b?c");
        assert_eq!(from.resolve("/b").unwrap().host, "github.com");
        assert_eq!(from.resolve("https://evil.example/x").unwrap().host, "evil.example");
        assert_eq!(from.resolve("//evil.example/x"), None, "a host in disguise");
        assert_eq!(from.resolve("b"), None);
    }
}
