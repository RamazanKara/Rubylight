//! The Rubylight 7.0 look of the console without scripts: the brand mark, the class of each form
//! button, and the live stream strip; its tests keep `style.css` on the frozen 7.0 tokens of
//! `rust/web/src/styles/tokens.css` and its text readable.
use super::{i18n, text};
use serde_json::Value;

/// The app icon's mark, a white "R" on a rounded ruby square, as `BrandMark.svelte` draws it;
/// `style.css` colours it from `--brand` and `--brand-ink`.
pub(super) const BRAND: &str = "<a class=\"brand\" href=\"/\"><svg class=\"mark\" viewBox=\"0 0 32 32\" aria-hidden=\"true\" focusable=\"false\"><rect width=\"32\" height=\"32\" rx=\"8\"/><text x=\"16\" y=\"23.5\" text-anchor=\"middle\">R</text></svg><span>Rubylight</span></a>";

/// Actions that remove something the user set up are drawn as the danger outline.
pub(super) fn button_class(op: &str) -> &'static str {
    match op {
        "unpair" | "app-delete" | "golden-delete" => "secondary danger",
        _ => "secondary",
    }
}

fn codec(format: &Value) -> Option<&'static str> {
    Some(match format.as_u64()? {
        0 => "H.264",
        1 => "HEVC",
        2 => "AV1",
        3 => "PyroWave",
        _ => return None,
    })
}

fn encoder(name: &str) -> &str {
    match name {
        "amf" => "AMD AMF",
        "nvenc" => "NVIDIA NVENC",
        "pyrowave" => "PyroWave",
        other => other,
    }
}

fn figure(out: &mut String, label: &str, value: &str) {
    *out += &format!(
        "<div><dt>{}</dt><dd>{}</dd></div>",
        i18n::message(label),
        i18n::data(value)
    );
}

/// One line per running stream on a dark media strip, from `/api/rtsp/sessions`. A figure the
/// host did not report, or has not measured yet, is left out rather than shown as zero.
pub(super) fn telemetry_strip(sessions: &[Value]) -> String {
    if sessions.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for session in sessions {
        let mut figures = String::new();
        match session["role"].as_str() {
            Some("input_only") => {
                figures += &format!(
                    "<div><dt>{}</dt><dd>{}</dd></div>",
                    i18n::message("Connection"),
                    i18n::message("Remote input")
                )
            }
            Some("remote_monitor") => {
                figures += &format!(
                    "<div><dt>{}</dt><dd>{}</dd></div>",
                    i18n::message("Connection"),
                    i18n::message("Remote monitor")
                )
            }
            _ => {}
        }
        if session["role"] != "input_only" {
            let size = (session["width"].as_u64(), session["height"].as_u64());
            if let (Some(width @ 1..), Some(height @ 1..)) = size {
                figure(&mut figures, "Resolution", &format!("{width}×{height}"));
            }
            if let Some(fps @ 1..) = session["fps"].as_u64() {
                figure(&mut figures, "Frame rate", &format!("{fps} fps"));
            }
            if let Some(name) = codec(&session["video_format"]) {
                let hdr = if session["hdr"] == true {
                    " · HDR"
                } else {
                    ""
                };
                figure(&mut figures, "Codec", &format!("{name}{hdr}"));
            }
            let performance = &session["performance"];
            let measured = performance["bitrate_mbps"].as_f64().filter(|v| *v > 0.);
            if let Some(rate) = measured {
                figure(&mut figures, "Bitrate", &format!("{rate:.1} Mbps"));
            } else if let Some(kbps @ 1..) = session["encoder_bitrate_kbps"].as_u64() {
                let mbps = kbps as f64 / 1000.;
                figure(&mut figures, "Target bitrate", &format!("{mbps:.1} Mbps"));
            }
            if let Some(ms) = performance["encode_p95_ms"].as_f64().filter(|v| *v > 0.) {
                figure(&mut figures, "Encoding time (p95)", &format!("{ms:.2} ms"));
            }
            if let Some(name) = session["encoder"].as_str().filter(|n| !n.is_empty()) {
                figure(&mut figures, "Encoder", encoder(name));
            }
        }
        rows += &format!(
            "<li><strong class=\"device\">{}</strong><dl>{figures}</dl></li>",
            i18n::data(text(session, "device_name"))
        );
    }
    format!(
        "<section class=\"telemetry\" aria-labelledby=\"live-now\"><h2 id=\"live-now\"><span class=\"live-dot\" aria-hidden=\"true\"></span>{}</h2><ul>{rows}</ul></section>",
        i18n::message("Streaming now")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension,
        body::to_bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, header},
    };
    use butterpollo_stream::state::test_support::Fixture;
    use serde_json::json;
    use std::collections::BTreeMap;

    const STYLE: &str = include_str!("style.css");
    const TOKENS: &str = include_str!("../../../../web/src/styles/tokens.css");
    const DARK: &str = ":root[data-theme='dark']";
    const SYSTEM: &str = ":root[data-theme='system']";
    const SYSTEM_MEDIA: &str = "@media (prefers-color-scheme: dark)";

    fn uncommented(css: &str) -> String {
        let mut out = String::new();
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            out += &rest[..start];
            rest = rest[start..]
                .split_once("*/")
                .map_or("", |(_, after)| after);
        }
        out + rest
    }
    /// The body of the first rule whose selector is exactly `selector`.
    fn block<'a>(css: &'a str, selector: &str) -> &'a str {
        let open = format!("{selector} {{");
        let start = css.find(&open).unwrap_or_else(|| panic!("no {selector}")) + open.len();
        let mut depth = 1;
        for (at, c) in css[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' if depth == 1 => return &css[start..start + at],
                '}' => depth -= 1,
                _ => {}
            }
        }
        panic!("{selector} is not closed")
    }
    fn properties(body: &str) -> BTreeMap<String, String> {
        body.split(';')
            .filter_map(|declaration| {
                let (name, value) = declaration.split_once(':')?;
                let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                name.trim()
                    .starts_with("--")
                    .then(|| (name.trim().to_owned(), value))
            })
            .collect()
    }
    /// The light, dark and system-dark custom properties of `css`.
    fn blocks(css: &str) -> [BTreeMap<String, String>; 3] {
        [
            properties(block(css, ":root")),
            properties(block(css, DARK)),
            properties(block(block(css, SYSTEM_MEDIA), SYSTEM)),
        ]
    }
    fn is_colour(value: &str) -> bool {
        value.contains('#') || value.contains("rgb(")
    }

    #[test]
    fn every_custom_property_carries_its_token_value() {
        let (style, tokens) = (uncommented(STYLE), uncommented(TOKENS));
        let ours = blocks(&style);
        let theirs = blocks(&tokens);
        for (name, (mine, token)) in ["light", "dark", "system dark"]
            .iter()
            .zip(ours.iter().zip(&theirs))
        {
            for (property, value) in mine {
                match token.get(property) {
                    Some(expected) => assert_eq!(value, expected, "{name} {property}"),
                    // The console's own names only alias a token.
                    None => assert!(
                        !is_colour(value) && value.starts_with("var(--"),
                        "{name} {property}: {value} is not a token"
                    ),
                }
            }
        }
        // Every light colour the dark tokens change is changed in both dark blocks.
        for (property, value) in &ours[0] {
            if is_colour(value) && theirs[1].contains_key(property) {
                assert!(ours[1].contains_key(property), "dark misses {property}");
            }
        }
        assert_eq!(ours[1], ours[2], "system dark must equal dark");
        assert!(ours[0].values().filter(|v| is_colour(v)).count() >= 30);
    }

    fn luminance(hex: &str) -> f64 {
        let hex = hex.strip_prefix('#').filter(|h| h.len() == 6);
        let hex = hex.unwrap_or_else(|| panic!("not a #rrggbb colour"));
        let channel = |at: usize| {
            let c = u8::from_str_radix(&hex[at..at + 2], 16).unwrap() as f64 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
    }
    fn contrast(a: &str, b: &str) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn the_pairs_the_stylesheet_draws_are_readable_in_both_themes() {
        let style = uncommented(STYLE);
        let [light, dark, _] = blocks(&style);
        let mut dark_theme = light.clone();
        dark_theme.extend(dark);
        let surfaces = ["--bg", "--panel", "--sunken"];
        let mut text: Vec<(&str, &str)> = ["--ink", "--ink-2", "--muted"]
            .iter()
            .flat_map(|fg| surfaces.iter().map(move |bg| (*fg, *bg)))
            .collect();
        text.extend([
            ("--ink", "--accent-soft"),
            ("--accent-text", "--panel"),
            ("--accent-text", "--bg"),
            ("--accent-ink", "--accent"),
            ("--accent-ink", "--accent-strong"),
            ("--brand-ink", "--brand"),
            ("--ok", "--ok-soft"),
            ("--warn", "--warn-soft"),
            ("--danger", "--danger-soft"),
            ("--danger", "--panel"),
            ("--media-ink", "--media-bg"),
            ("--media-ink-2", "--media-bg"),
        ]);
        // Focus rings, field outlines (--muted), the danger outline and the primary fill.
        let mut outlines: Vec<(&str, &str)> = ["--focus", "--muted"]
            .iter()
            .flat_map(|fg| surfaces.iter().map(move |bg| (*fg, *bg)))
            .collect();
        outlines.extend([
            ("--danger", "--panel"),
            ("--accent", "--panel"),
            ("--accent", "--bg"),
            ("--media-ok", "--media-bg"),
        ]);
        let mut failures = Vec::new();
        for (name, theme) in [("light", &light), ("dark", &dark_theme)] {
            for (pairs, minimum) in [(&text, 4.5), (&outlines, 3.)] {
                for (fg, bg) in pairs.iter() {
                    let ratio = contrast(&theme[*fg], &theme[*bg]);
                    if ratio < minimum {
                        failures.push(format!("{name} {fg} on {bg}: {ratio:.2}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn no_raw_colour_outside_the_custom_property_blocks() {
        let style = uncommented(STYLE);
        let mut rest = style.clone();
        for body in [
            block(&style, ":root"),
            block(&style, DARK),
            block(block(&style, SYSTEM_MEDIA), SYSTEM),
        ] {
            rest = rest.replacen(body, "", 1);
        }
        let hex = rest.match_indices('#').find(|(at, _)| {
            let digits = rest[at + 1..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .count();
            (3..=8).contains(&digits)
        });
        assert!(hex.is_none(), "raw colour at {hex:?}");
        assert!(!rest.contains("rgb("), "raw colour outside the blocks");
    }

    #[test]
    fn the_strip_shows_only_what_the_host_reports() {
        assert_eq!(telemetry_strip(&[]), "");
        let full = json!({"device_name":"<b>Living room</b>","width":2560,"height":1440,"fps":120,
            "video_format":1,"hdr":true,"encoder":"amf","encoder_bitrate_kbps":60000,
            "performance":{"bitrate_mbps":48.24,"encode_p95_ms":3.214}});
        let html = i18n::render(&telemetry_strip(&[full]), "en");
        for expected in [
            "2560×1440",
            "120 fps",
            "HEVC · HDR",
            "48.2 Mbps",
            "3.21 ms",
            "AMD AMF",
            "&lt;b&gt;Living room",
            "Streaming now",
        ] {
            assert!(html.contains(expected), "{expected}: {html}");
        }
        assert!(!html.contains("Target bitrate"), "{html}");
        let bare = json!({"device_name":"Phone","width":0,"height":0,"fps":0,"encoder":"",
            "performance":{"fps":0.0,"bitrate_mbps":0.0,"encode_p95_ms":0.0}});
        let html = i18n::render(&telemetry_strip(&[bare]), "en");
        for absent in [
            "Resolution",
            "Frame rate",
            "Bitrate",
            "Encoding",
            "Codec",
            "Encoder",
        ] {
            assert!(!html.contains(absent), "{absent}: {html}");
        }
        let input = json!({"device_name":"Pad","role":"input_only","video_format":0,"fps":60});
        let html = i18n::render(&telemetry_strip(&[input]), "de");
        assert!(
            !html.contains("H.264") && !html.contains("60 fps"),
            "{html}"
        );
        let target = json!({"device_name":"TV","encoder_bitrate_kbps":20000});
        let html = i18n::render(&telemetry_strip(&[target]), "en");
        assert!(
            html.contains("Target bitrate") && html.contains("20.0 Mbps"),
            "{html}"
        );
    }

    /// The fixture with the test administrator signed up, as the API tests have it.
    async fn configured() -> Fixture {
        let f = Fixture::new();
        crate::test_support::request(&f, "GET", "/api/config", Value::Null).await;
        f
    }
    async fn render(f: &Fixture, uri: &str, signed_in: bool) -> String {
        let mut headers = HeaderMap::new();
        if signed_in {
            let (access, ..) = f
                .host
                .new_web_session(
                    "parity".into(),
                    false,
                    "test".into(),
                    "127.0.0.1".into(),
                    None,
                )
                .unwrap();
            let cookie = format!("__Host-apollo_session={access}");
            headers.insert(header::COOKIE, cookie.parse().unwrap());
        }
        let response = super::super::page(
            State(f.host.clone()),
            Extension(f.connection(true)),
            Method::GET,
            uri.parse().unwrap(),
            headers,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }
    fn set_locale(f: &Fixture, locale: &str) {
        let mut config = f.host.config.write().unwrap();
        config.values.insert("locale".into(), locale.into());
    }
    fn word(html: &str, name: &str) -> bool {
        html.match_indices(name).any(|(at, _)| {
            let before = html[..at].chars().next_back();
            let after = html[at + name.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    }
    const PAGES: [&str; 8] = [
        "/",
        "/library",
        "/devices",
        "/settings",
        "/logs",
        "/integrations",
        "/api-tokens",
        "/maintenance",
    ];

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_page_says_experimental_or_names_a_mod() {
        let unconfigured = Fixture::new();
        let mut pages = vec![("/setup", render(&unconfigured, "/setup", false).await)];
        let f = configured().await;
        // A paired device and a running stream bring in the device settings and the strip.
        let client = f.client(u32::MAX);
        f.session(client, butterpollo_session::session::Role::Stream);
        for locale in ["en", "de", "ru"] {
            set_locale(&f, locale);
            pages.push(("/login", render(&f, "/login", false).await));
            for path in PAGES {
                pages.push((path, render(&f, path, true).await));
            }
        }
        for (path, html) in &pages {
            assert!(
                !html.to_lowercase().contains("experimental"),
                "{path} says experimental"
            );
            assert!(!html.contains("Nucleus") && !word(html, "ANR"), "{path}");
        }
        let devices = &pages.iter().find(|(p, _)| *p == "/devices").unwrap().1;
        assert!(devices.contains("Stream in the background (co-op splitter, controllers only)"));
        let live = &pages.iter().find(|(p, _)| *p == "/").unwrap().1;
        assert!(live.contains("class=\"telemetry\""), "{live}");
    }

    /// Writes each page with its stylesheet for a look in a browser: set
    /// `RUBYLIGHT_CONSOLE_DUMP_DIR` to a directory, otherwise this does nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dump_pages_for_review() {
        let Some(directory) = std::env::var_os("RUBYLIGHT_CONSOLE_DUMP_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("console.css"), STYLE).unwrap();
        let write = |name: &str, html: String| {
            let html = html.replace("href=\"/console.css\"", "href=\"console.css\"");
            std::fs::write(directory.join(format!("{name}.html")), html).unwrap();
        };
        let f = configured().await;
        write("sign-in", render(&f, "/login", false).await);
        write("overview-idle", render(&f, "/", true).await);
        let client = f.client(0x0100_0000 | 0x0400_0000 | 0x0000_0100);
        let log = butterpollo_stream::maintenance::log_path(&f.host);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "2026-11-26 18:02:11 INFO host started, version 7.0.0\n2026-11-26 18:02:12 INFO encoder check: H.264 ready, HEVC ready, AV1 ready\n2026-11-26 18:04:40 INFO stream started for Parity device: 2560x1440 at 120 fps, HEVC, HDR\n").unwrap();
        write("apps", render(&f, "/library", true).await);
        write("devices", render(&f, "/devices", true).await);
        write("settings", render(&f, "/settings", true).await);
        write("logs", render(&f, "/logs", true).await);
        let negotiated = butterpollo_media_policy::rtsp::Negotiated {
            width: 2560,
            height: 1440,
            fps: 120,
            bitrate_kbps: 60_000,
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        let launch = f.launch(client, butterpollo_session::session::Role::Stream);
        let session = butterpollo_stream::state::Session::new(launch, negotiated);
        *session.encoder.write().unwrap() = "amf".into();
        let id = session.launch.id.clone();
        f.host.sessions.lock().unwrap().active.insert(id, session);
        write("overview-live", render(&f, "/", true).await);
        for locale in ["de", "ru"] {
            set_locale(&f, locale);
            write(
                &format!("overview-live-{locale}"),
                render(&f, "/", true).await,
            );
            write(
                &format!("settings-{locale}"),
                render(&f, "/settings", true).await,
            );
        }
    }
}
