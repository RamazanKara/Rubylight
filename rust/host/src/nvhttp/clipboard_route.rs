//! `/actions/clipboard`: reading and writing the PC's clipboard, text and images, within their limits.

use super::*;

/// What `/actions/clipboard` carries: `type=text`, or `type=png` while the
/// host's `clipboard_image` setting is on (`docs/native-api.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ClipboardType {
    Text,
    Png,
}
fn clipboard_type(args: &Args) -> Option<ClipboardType> {
    match args.get("type").map(String::as_str) {
        Some("text") => Some(ClipboardType::Text),
        Some("png") => Some(ClipboardType::Png),
        _ => None,
    }
}
/// The longest body a device may send for `kind`: the 1 MiB of the other
/// routes for text, `clipboard_image_max_mb` for an image.
fn clipboard_limit(h: &Shared, kind: Option<ClipboardType>) -> usize {
    match kind {
        Some(ClipboardType::Png) => {
            let max = h.config.read().unwrap().clipboard_image_max_bytes();
            usize::try_from(max).unwrap_or(usize::MAX)
        }
        _ => 1024 * 1024,
    }
}
/// What a refused body of `kind` is called in the log.
fn clipboard_what(kind: Option<ClipboardType>) -> &'static str {
    match kind {
        Some(ClipboardType::Png) => "an image",
        _ => "a copy",
    }
}
/// How often a device that isn't paired gets a refusal logged.
pub(super) const UNPAIRED_REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(60);
/// How many addresses get a refusal logged in one interval.
pub(super) const UNPAIRED_REFUSAL_LOG_PEERS: usize = 64;
/// Size refusals of devices that aren't paired, refused before they are
/// authenticated: logged once per address in [`UNPAIRED_REFUSAL_LOG_INTERVAL`]
/// (IPv6 per /64, as a device's privacy addresses rotate within it), for at
/// most [`UNPAIRED_REFUSAL_LOG_PEERS`] addresses, so one on the network can't
/// fill the log.
pub(super) struct RefusalLog(pub(super) Mutex<BTreeMap<std::net::IpAddr, (Instant, u32)>>);
static UNPAIRED_REFUSALS: RefusalLog = RefusalLog::new();
impl RefusalLog {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(BTreeMap::new()))
    }
    /// Whether `peer`'s refusal at `now` is logged: with the count left
    /// unlogged since its last line, or `None` while it is suppressed too.
    pub(super) fn admit(&self, peer: std::net::IpAddr, now: Instant) -> Option<u32> {
        let peer = match peer.to_canonical() {
            std::net::IpAddr::V6(v6) => {
                std::net::IpAddr::V6((u128::from(v6) & (u128::MAX << 64)).into())
            }
            v4 => v4,
        };
        let mut logged = self.0.lock().unwrap();
        if let Some((at, suppressed)) = logged.get_mut(&peer)
            && now.saturating_duration_since(*at) < UNPAIRED_REFUSAL_LOG_INTERVAL
        {
            *suppressed = suppressed.saturating_add(1);
            return None;
        }
        let suppressed = logged.remove(&peer).map_or(0, |(_, suppressed)| suppressed);
        // Only addresses refused within the interval are held.
        logged.retain(|_, (at, _)| {
            now.saturating_duration_since(*at) < UNPAIRED_REFUSAL_LOG_INTERVAL
        });
        if logged.len() >= UNPAIRED_REFUSAL_LOG_PEERS {
            return None;
        }
        logged.insert(peer, (now, 0));
        Some(suppressed)
    }
}
/// Whether `c` comes from a paired, enabled device, as [`authenticated`]
/// finds it, without noting a request.
fn paired_connection(h: &Shared, c: &Connection) -> bool {
    c.tls
        && c.certificate.as_deref().is_some_and(|der| {
            h.paired
                .read()
                .unwrap()
                .client_by_certificate(der)
                .is_some()
        })
}
/// Image clipboard requests take turns: each holds up to the image limit as
/// its body or copy, and far more while it converts, so they would add up.
/// A device asking again before its last image request has ended is
/// refused, so one device can't queue ahead of the others.
pub(super) struct ImageTurns {
    pub(super) devices: Mutex<BTreeSet<String>>,
    turn: tokio::sync::Semaphore,
}
pub(super) static IMAGE_TURNS: ImageTurns = ImageTurns::new();
/// A device's image request, holding the turn until it is dropped.
pub(super) struct ImageTurn<'a> {
    _turn: tokio::sync::SemaphorePermit<'a>,
    _device: ImageDevice<'a>,
}
struct ImageDevice<'a>(&'a ImageTurns, String);
impl Drop for ImageDevice<'_> {
    fn drop(&mut self) {
        self.0.devices.lock().unwrap().remove(&self.1);
    }
}
impl ImageTurns {
    pub(super) const fn new() -> Self {
        Self {
            devices: Mutex::new(BTreeSet::new()),
            turn: tokio::sync::Semaphore::const_new(1),
        }
    }
    /// The turn of `device`, after any other image request has ended, or
    /// `None` while one of its own is waiting or running.
    pub(super) async fn take(&self, device: &str) -> Option<ImageTurn<'_>> {
        if !self.devices.lock().unwrap().insert(device.to_owned()) {
            return None;
        }
        // Released if the request is dropped while it waits.
        let device = ImageDevice(self, device.to_owned());
        let turn = self
            .turn
            .acquire()
            .await
            .expect("the image turn is never closed");
        Some(ImageTurn {
            _turn: turn,
            _device: device,
        })
    }
}
pub(super) fn clipboard_client(
    h: &Shared,
    c: &Connection,
    permission: u32,
    args: &Args,
) -> Result<(ClipboardType, String)> {
    let client = authenticated(h, c, permission)?;
    if client.perm & ((1 << 25) | (1 << 26)) == 0 {
        bail!("view permission required");
    }
    let kind = match clipboard_type(args) {
        Some(ClipboardType::Png) if !h.config.read().unwrap().clipboard_image() => {
            bail!("the image clipboard is off on this host")
        }
        Some(kind) => kind,
        None => bail!("only text and PNG clipboard data are supported"),
    };
    let sessions: Vec<_> = h
        .sessions
        .lock()
        .unwrap()
        .active
        .values()
        .filter(|s| s.launch.client.uuid == client.uuid && !s.stopping())
        .cloned()
        .collect();
    if sessions.is_empty() {
        bail!("clipboard access requires an active session");
    }
    // The clipboard belongs to the person using the PC, not to a device
    // playing in the background.
    if sessions.iter().any(|s| {
        crate::stream::effective_config(h, &s.launch).map_or(true, |config| {
            butterpollo_core::background::enabled(&config) || crate::nucleus::player(&config)
        })
    }) {
        bail!("clipboard access is off for background streams");
    }
    Ok((kind, client.uuid))
}
pub(super) async fn clipboard_read(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    let Ok((kind, device)) = clipboard_client(&h, &c, 1 << 17, &args) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if kind == ClipboardType::Png {
        let Some(turn) = IMAGE_TURNS.take(&device).await else {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        };
        let max = clipboard_limit(&h, Some(kind));
        // The turn goes with the copy, which outlives a dropped request.
        let read = move || {
            let _turn = turn;
            clipboard::read_png(max)
        };
        return match tokio::task::spawn_blocking(read).await {
            Ok(Ok(Some(png))) => ([(header::CONTENT_TYPE, "image/png")], png).into_response(),
            Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
            // The device did nothing wrong: an image this host can't
            // convert is 415 whatever is wrong with it.
            Ok(Err(error)) => {
                clipboard_image_failure("reading", &error, StatusCode::UNSUPPORTED_MEDIA_TYPE)
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    match tokio::task::spawn_blocking(clipboard::read).await {
        Ok(Ok(text)) => {
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
/// The body takes no more memory than the type's limit: a longer one is 413
/// before it is read, if it says so, and once it passes the limit if not.
/// An image's dimensions are then checked before its pixels are decoded
/// (`butterpollo_core::clipboard_image`).
pub(super) async fn clipboard_write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
    request: axum::extract::Request,
) -> Response {
    // Refused before anything else, as the 1 MiB request limit was.
    let limit = clipboard_limit(&h, clipboard_type(&args));
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.trim().parse::<u64>().ok());
    if let Some(length) = declared.filter(|length| *length > limit as u64) {
        let what = clipboard_what(clipboard_type(&args));
        if paired_connection(&h, &c) {
            tracing::warn!(
                "Clipboard: refused {what} of {length} bytes by its Content-Length, over the limit of {limit} bytes"
            );
        } else if let Some(suppressed) = UNPAIRED_REFUSALS.admit(c.peer.ip(), Instant::now()) {
            tracing::warn!(
                "Clipboard: refused {what} of {length} bytes by its Content-Length from unpaired {}, over the limit of {limit} bytes ({suppressed} more from it unlogged)",
                c.peer.ip().to_canonical()
            );
        }
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let Ok((kind, device)) = clipboard_client(&h, &c, 1 << 16, &args) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if kind == ClipboardType::Png && !is_png_content_type(request.headers()) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    // An image's turn comes before its body is read.
    let turn = match kind {
        ClipboardType::Png => match IMAGE_TURNS.take(&device).await {
            Some(turn) => Some(turn),
            None => return StatusCode::TOO_MANY_REQUESTS.into_response(),
        },
        ClipboardType::Text => None,
    };
    let body = match body_within(request.into_body(), limit).await {
        Ok(body) => body,
        Err(status) => {
            if status == StatusCode::PAYLOAD_TOO_LARGE {
                tracing::warn!(
                    "Clipboard: refused {} whose body passed the limit of {limit} bytes",
                    clipboard_what(Some(kind))
                );
            }
            return status.into_response();
        }
    };
    if kind == ClipboardType::Png {
        let write = move || {
            let _turn = turn;
            clipboard::write_png(&body)
        };
        return match tokio::task::spawn_blocking(write).await {
            Ok(Ok(())) => StatusCode::OK.into_response(),
            Ok(Err(error)) => clipboard_image_failure("writing", &error, StatusCode::BAD_REQUEST),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    let Ok(text) = String::from_utf8(body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match tokio::task::spawn_blocking(move || clipboard::write(&text)).await {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
fn is_png_content_type(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("image/png"))
}
/// A request body of at most `limit` bytes: 413 once it passes that, 400
/// when it can't be read.
async fn body_within(mut body: axum::body::Body, limit: usize) -> Result<Vec<u8>, StatusCode> {
    use axum::body::HttpBody;
    let mut bytes = Vec::new();
    while let Some(frame) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
    {
        let Ok(data) = frame.map_err(|_| StatusCode::BAD_REQUEST)?.into_data() else {
            continue;
        };
        if bytes.len() + data.len() > limit {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        bytes.extend_from_slice(&data);
    }
    Ok(bytes)
}
/// 413 for an image over the limit, `malformed` for one that can't be
/// converted, 500 when the clipboard itself failed. Each is logged.
fn clipboard_image_failure(doing: &str, error: &anyhow::Error, malformed: StatusCode) -> Response {
    use butterpollo_core::clipboard_image::ImageError;
    let status = match error.downcast_ref::<ImageError>() {
        Some(ImageError::TooLarge | ImageError::TooManyBytes { .. }) => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        Some(ImageError::Unsupported(_)) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Some(ImageError::NotPng | ImageError::Malformed(_)) => malformed,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    };
    tracing::warn!("Clipboard: {doing} an image failed ({status}): {error:#}");
    status.into_response()
}
