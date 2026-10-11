//! Host configuration and metadata API handlers.

use crate::state::Shared;
use butterpollo_core::state;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/config") => {
            let c = if h.port_override.is_some() {
                butterpollo_core::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            let mut v = c.json();
            v["status"] = json!(true);
            v["platform"] = json!("windows");
            v["version"] = json!(env!("CARGO_PKG_VERSION"));
            v
        }
        ("POST" | "PATCH", "/api/config") => {
            let mut object = data
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("configuration must be an object"))?
                .clone();
            for key in ["status", "platform", "version"] {
                object.remove(key);
            }
            let _save = h.config_save.lock().unwrap();
            let mut next = if h.port_override.is_some() {
                butterpollo_core::config::Config::load(&h.config_path)?
            } else {
                h.config.read().unwrap().clone()
            };
            next.update(&object)?;
            next.check_saved(object.keys())?;
            state::atomic_write(&h.config_path, next.text().as_bytes())?;
            let warning =
                butterpollo_windows::vulkan::reconcile(next.boolean("vulkan_hdr_layer", true))
                    .err()
                    .map(|e| e.to_string());
            crate::logging::apply(next.log_level());
            if let Some(port) = h.port_override {
                next.values.insert("port".into(), port.to_string());
            }
            *h.config.write().unwrap() = next;
            h.config_version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            h.metadata.lock().unwrap().take();
            json!({"status":true,"restart_required":true,"warning":warning})
        }
        ("GET", "/api/configLocale") => {
            json!({"status":true,"locale":h.config.read().unwrap().get("locale","en")})
        }
        ("GET", "/api/meta" | "/api/metadata") => {
            let shared = h.clone();
            (|| -> anyhow::Result<Value> {
                let mut cached = shared.metadata.lock().unwrap();
                if let Some((checked, value)) = cached.as_ref()
                    && checked.elapsed() < Duration::from_secs(5)
                {
                    return Ok(value.clone());
                }
                let _com = butterpollo_windows::capture::ComGuard::new()?;
                let codecs = shared.codecs.load(std::sync::atomic::Ordering::Acquire);
                let probing = shared
                    .probing_codecs
                    .load(std::sync::atomic::Ordering::Acquire);
                let virtual_display = butterpollo_windows::display::virtual_display_status();
                let capable = virtual_display["capable"].as_bool().unwrap_or(false);
                let audio = butterpollo_windows::audio_route::endpoints();
                let displays = butterpollo_windows::capture::displays();
                let config = shared.config.read().unwrap().clone();
                let port = config.ports()?.http;
                let address = |host: String| {
                    if port == 47989 {
                        host
                    } else {
                        format!("{host}:{port}")
                    }
                };
                let addresses = butterpollo_windows::net::lan_addresses()
                    .unwrap_or_default()
                    .into_iter()
                    .map(address)
                    .collect::<Vec<_>>();
                let pc_address = addresses
                    .first()
                    .cloned()
                    .unwrap_or_else(|| address(std::env::var("COMPUTERNAME").unwrap_or_default()));
                let value = json!({"status":true,"platform":"windows","version":env!("CARGO_PKG_VERSION"),"branch":"codex/butterpollo-rust","host_name":crate::network::host_name(&config),"pc_address":pc_address,"pc_addresses":addresses,"paired_devices":shared.paired.read().unwrap().clients.len(),"enable_pairing":config.boolean("enable_pairing",true),"warnings":shared.warnings.snapshot(),"encoder_status":{"state":if probing {"checking"} else if codecs == 0 {"failed"} else {"ready"},"h264":codecs&1!=0,"hevc":codecs&0x100!=0,"av1":codecs&0x10000!=0,"pyrowave":codecs&0x800000!=0},"capture_status":{"configured_backend":config.get("capture","auto"),"virtual_display_configured":config.virtual_display_mode(butterpollo_windows::display::windows_11())!="disabled","displays":displays.as_ref().ok(),"error":displays.as_ref().err().map(|e|e.to_string())},"virtual_display":virtual_display,"audio_sinks":audio.as_ref().ok(),"audio_error":audio.as_ref().err().map(|e|e.to_string()),"audio_enabled":config.boolean("stream_audio",true),"features":{"rust_host":true,"hdr":true,"truehdr_runtime":butterpollo_windows::truehdr::available(),"pyrowave":codecs&0x800000!=0,"virtual_display":capable},"credentials_exists":shared.credentials.read().unwrap().is_some()});
                *cached = Some((Instant::now(), value.clone()));
                Ok(value)
            })()?
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Host, test_support::Fixture};
    use butterpollo_core::config::Config;

    #[test]
    fn concurrent_configuration_patches_preserve_both_updates() {
        let fixture = Fixture::new();
        let barrier = std::sync::Barrier::new(2);
        let patches = [
            json!({"sunshine_name": "Concurrent host"}),
            json!({"locale": "de"}),
        ];
        std::thread::scope(|scope| {
            let workers: Vec<_> = patches
                .iter()
                .map(|patch| {
                    let barrier = &barrier;
                    let host = &fixture.host;
                    scope.spawn(move || {
                        barrier.wait();
                        handle(host, "PATCH", "/api/config", patch).unwrap();
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let saved = Config::load(&fixture.host.config_path).unwrap();
        assert_eq!(saved.get("sunshine_name", ""), "Concurrent host");
        assert_eq!(saved.get("locale", ""), "de");
        assert_eq!(fixture.host.config.read().unwrap().values, saved.values);
    }

    #[test]
    fn saving_configuration_does_not_persist_the_command_line_port() {
        let fixture = Fixture::new();
        std::fs::write(&fixture.host.config_path, "port = 48000\n").unwrap();
        let host = Host::load(
            fixture.host.directory.clone(),
            fixture.host.assets.clone(),
            Some(48518),
        )
        .unwrap();
        assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
        let mut saved = handle(&host, "GET", "/api/config", &Value::Null).unwrap();
        assert_eq!(saved["port"], 48000);
        saved["sunshine_name"] = "Test host".into();
        handle(&host, "POST", "/api/config", &saved).unwrap();
        assert_eq!(
            Config::load(&host.config_path).unwrap().port().unwrap(),
            48000
        );
        assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
        saved["port"] = "48100".into();
        handle(&host, "POST", "/api/config", &saved).unwrap();
        assert_eq!(
            Config::load(&host.config_path).unwrap().port().unwrap(),
            48100
        );
        assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
    }
}
