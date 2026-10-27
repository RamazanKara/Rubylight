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
            // The Webhooks page manages them, and their secrets never come back.
            if let Some(values) = v.as_object_mut() {
                values.remove("webhooks");
            }
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
            if object.contains_key("webhooks") {
                anyhow::bail!(
                    "webhooks are saved on the Webhooks page (/api/webhooks), which keeps their secrets"
                );
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
            // The launchers to read changed: the last scan answers for the old ones.
            if object.keys().any(|key| key.starts_with("library_")) {
                crate::library::invalidate(h);
            }
            json!({"status":true,"restart_required":true,"warning":warning})
        }
        ("GET", "/api/configLocale") => {
            json!({"status":true,"locale":h.config.read().unwrap().get("locale","en")})
        }
        ("GET", "/api/meta" | "/api/metadata") => metadata(
            h,
            butterpollo_windows::capture::gpus,
            butterpollo_windows::capture::displays,
        )?,
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

/// Saves one setting that a page manages itself: `next` gets the stored text
/// (None when unset) and returns the text to save, or an error that saves
/// nothing. Serialized with the console's other saves.
pub(super) fn save_setting(
    h: &Shared,
    key: &str,
    next: impl FnOnce(Option<&str>) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let _save = h.config_save.lock().unwrap();
    let mut config = if h.port_override.is_some() {
        butterpollo_core::config::Config::load(&h.config_path)?
    } else {
        h.config.read().unwrap().clone()
    };
    let value = next(config.values.get(key).map(String::as_str))?;
    config.values.insert(key.to_owned(), value);
    config.check_saved([&key.to_owned()])?;
    state::atomic_write(&h.config_path, config.text().as_bytes())?;
    if let Some(port) = h.port_override {
        config.values.insert("port".into(), port.to_string());
    }
    *h.config.write().unwrap() = config;
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn metadata(
    shared: &Shared,
    gpu_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_windows::capture::Gpu>>,
    display_inventory: impl FnOnce() -> anyhow::Result<Vec<butterpollo_windows::capture::Display>>,
) -> anyhow::Result<Value> {
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
    let gpus = gpu_inventory();
    let displays = display_inventory();
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
    let value = json!({"status":true,"platform":"windows","version":env!("CARGO_PKG_VERSION"),"branch":"codex/butterpollo-rust","host_name":crate::network::host_name(&config),"pc_address":pc_address,"pc_addresses":addresses,"paired_devices":shared.paired.read().unwrap().clients.len(),"enable_pairing":config.boolean("enable_pairing",true),"qr_pairing":config.qr_pairing(),"warnings":shared.warnings.snapshot(),"encoder_status":{"state":if probing {"checking"} else if codecs == 0 {"failed"} else {"ready"},"h264":codecs&1!=0,"hevc":codecs&0x100!=0,"av1":codecs&0x10000!=0,"pyrowave":codecs&0x800000!=0},"capture_status":{"configured_backend":config.get("capture","auto"),"virtual_display_configured":config.virtual_display_mode(butterpollo_windows::display::virtual_display_os())!="disabled","displays":displays.as_ref().ok(),"error":displays.as_ref().err().map(|e|e.to_string())},"virtual_display":virtual_display,"gpus":gpus.as_ref().ok(),"gpu_error":gpus.as_ref().err().map(|e|e.to_string()),"audio_sinks":audio.as_ref().ok(),"audio_error":audio.as_ref().err().map(|e|e.to_string()),"audio_enabled":config.boolean("stream_audio",true),"features":{"rust_host":true,"hdr":true,"pyrowave":codecs&0x800000!=0,"virtual_display":capable},"credentials_exists":shared.credentials.read().unwrap().is_some(),"webrtc":shared.webrtc.status_json(&config)});
    *cached = Some((Instant::now(), value.clone()));
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Host, test_support::Fixture};
    use butterpollo_core::config::Config;
    use butterpollo_windows::capture::{Display, Gpu};

    fn hybrid_gpus() -> Vec<Gpu> {
        vec![
            Gpu {
                name: "AMD Radeon(TM) 780M Graphics".into(),
                vendor: 0x1002,
                dedicated_memory: 512 * 1024 * 1024,
                luid: (1, 0),
                pnp_id: Some("PCI\\VEN_1002&DEV_15BF\\1".into()),
            },
            Gpu {
                name: "AMD Radeon RX 7600M XT".into(),
                vendor: 0x1002,
                dedicated_memory: 8 * 1024 * 1024 * 1024,
                luid: (2, 0),
                pnp_id: None,
            },
        ]
    }

    fn integrated_display() -> Vec<Display> {
        vec![Display {
            device_id: r"\\.\DISPLAY1".into(),
            display_name: r"\\.\DISPLAY1".into(),
            friendly_name: "Laptop panel".into(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            primary: true,
            adapter: "AMD Radeon(TM) 780M Graphics".into(),
            adapter_index: 0,
            output_index: 0,
        }]
    }

    #[test]
    fn metadata_lists_render_only_gpus_independently_of_display_adapters() {
        let fixture = Fixture::new();
        let value = metadata(
            &fixture.host,
            || Ok(hybrid_gpus()),
            || Ok(integrated_display()),
        )
        .unwrap();
        assert_eq!(value["status"], true);
        assert_eq!(value["gpus"], json!(hybrid_gpus()));
        assert!(value["gpu_error"].is_null());
        assert_eq!(
            value["capture_status"]["displays"],
            json!(integrated_display())
        );
        assert!(value["capture_status"]["error"].is_null());
        let cached = metadata(
            &fixture.host,
            || panic!("a cached request must not enumerate GPUs again"),
            || panic!("a cached request must not enumerate displays again"),
        )
        .unwrap();
        assert_eq!(cached, value);
    }

    #[test]
    fn metadata_keeps_gpu_and_display_enumeration_failures_separate() {
        let fixture = Fixture::new();
        let missing_displays = metadata(
            &fixture.host,
            || Ok(hybrid_gpus()),
            || anyhow::bail!("display enumeration failed"),
        )
        .unwrap();
        assert_eq!(missing_displays["gpus"], json!(hybrid_gpus()));
        assert!(missing_displays["gpu_error"].is_null());
        assert!(missing_displays["capture_status"]["displays"].is_null());
        assert_eq!(
            missing_displays["capture_status"]["error"],
            "display enumeration failed"
        );

        fixture.host.metadata.lock().unwrap().take();
        let missing_gpus = metadata(
            &fixture.host,
            || anyhow::bail!("GPU enumeration failed"),
            || Ok(integrated_display()),
        )
        .unwrap();
        assert!(missing_gpus["gpus"].is_null());
        assert_eq!(missing_gpus["gpu_error"], "GPU enumeration failed");
        assert_eq!(
            missing_gpus["capture_status"]["displays"],
            json!(integrated_display())
        );
        assert!(missing_gpus["capture_status"]["error"].is_null());
    }

    #[test]
    fn metadata_distinguishes_empty_gpu_inventory_from_an_enumeration_failure() {
        let fixture = Fixture::new();
        let value = metadata(&fixture.host, || Ok(vec![]), || Ok(vec![])).unwrap();
        assert_eq!(value["gpus"], json!([]));
        assert!(value["gpu_error"].is_null());
        assert_eq!(value["capture_status"]["displays"], json!([]));
        assert!(value["capture_status"]["error"].is_null());
    }

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
    fn saving_a_library_setting_scans_the_launchers_again() {
        use butterpollo_core::native::Scan;
        let fixture = Fixture::new();
        let host = &fixture.host;
        let seeded = Scan::new(["epic:kept".to_owned()]);
        crate::library::seed(host, seeded.clone());
        handle(host, "PATCH", "/api/config", &json!({"locale": "de"})).unwrap();
        assert_eq!(
            crate::library::installed(host),
            Some(seeded.clone()),
            "other settings keep the scan"
        );
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"library_epic_enabled": false}),
        )
        .unwrap();
        // Tests scan nothing, so the new scan replaces the seeded one with an empty one.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while crate::library::installed(host) == Some(seeded.clone()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the library setting did not start a new scan"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn the_stream_preset_round_trips_without_saving_the_values_it_fills() {
        let fixture = Fixture::new();
        let host = &fixture.host;
        let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
        assert!(shown.get("stream_preset").is_none());
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"stream_preset": "reference"}),
        )
        .unwrap();
        let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
        assert_eq!(shown["stream_preset"], "reference");
        // The preset's values are not written to the file, so changing the
        // preset later changes them; only the name is saved.
        assert!(shown.get("amd_max_frame_size").is_none());
        let saved = Config::load(&host.config_path).unwrap();
        assert_eq!(saved.get("stream_preset", ""), "reference");
        assert!(!saved.values.contains_key("amd_max_frame_size"));
        // A manual value saves next to it and stays after the preset changes.
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"amd_max_frame_size": "2", "stream_preset": "lowest_latency"}),
        )
        .unwrap();
        let saved = Config::load(&host.config_path).unwrap();
        assert_eq!(saved.get("stream_preset", ""), "lowest_latency");
        assert_eq!(saved.get("amd_max_frame_size", ""), "2");
        // An unknown name is refused and the saved one stays.
        assert!(
            handle(
                host,
                "PATCH",
                "/api/config",
                &json!({"stream_preset": "fastest"})
            )
            .is_err()
        );
        let saved = Config::load(&host.config_path).unwrap();
        assert_eq!(saved.get("stream_preset", ""), "lowest_latency");
        // Clearing it means Balanced again.
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"stream_preset": null}),
        )
        .unwrap();
        let saved = Config::load(&host.config_path).unwrap();
        assert!(!saved.values.contains_key("stream_preset"));
        assert_eq!(saved.get("amd_max_frame_size", ""), "2");
    }

    #[test]
    fn the_console_gets_settings_saved_under_an_earlier_name_under_the_current_one() {
        let fixture = Fixture::new();
        std::fs::write(
            &fixture.host.config_path,
            "rtss_path = D:\\RTSS\ndd_vdd_static_monitor_count = 2\nrtx_hdr_peak_brightness = 1200\nfuture_setting_extra = kept\n",
        )
        .unwrap();
        let host = Host::load(
            fixture.host.directory.clone(),
            fixture.host.assets.clone(),
            None,
        )
        .unwrap();
        let shown = handle(&host, "GET", "/api/config", &Value::Null).unwrap();
        assert_eq!(shown["hdr_peak_brightness"], 1200);
        assert_eq!(shown["rtss_install_path"], "D:\\RTSS");
        assert_eq!(shown["dd_virtual_display_permanent_count"], 2);
        assert_eq!(shown["future_setting_extra"], "kept");
        for old in [
            "rtx_hdr_peak_brightness",
            "rtss_path",
            "dd_vdd_static_monitor_count",
        ] {
            assert!(shown.get(old).is_none(), "{old}");
        }
        // Loading leaves the file alone; the console's first save writes the
        // current names, so turning the setting off in the console turns it
        // off, and a later reset cannot bring back the earlier value.
        let saved = std::fs::read_to_string(&host.config_path).unwrap();
        assert!(saved.contains("dd_vdd_static_monitor_count"), "{saved}");
        handle(
            &host,
            "PATCH",
            "/api/config",
            &json!({"dd_virtual_display_permanent_count": 0}),
        )
        .unwrap();
        let saved = std::fs::read_to_string(&host.config_path).unwrap();
        assert!(!saved.contains("dd_vdd_static_monitor_count"), "{saved}");
        let reloaded = Config::load(&host.config_path).unwrap();
        assert_eq!(
            reloaded.integer("dd_virtual_display_permanent_count", -1),
            0
        );
        assert_eq!(reloaded.integer("hdr_peak_brightness", 0), 1200);
        handle(
            &host,
            "PATCH",
            "/api/config",
            &json!({"dd_virtual_display_permanent_count": null}),
        )
        .unwrap();
        let reset = Config::load(&host.config_path).unwrap();
        assert!(
            !reset
                .values
                .contains_key("dd_virtual_display_permanent_count")
        );
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
