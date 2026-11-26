//! Fallback HTML settings forms and form-submission parsing, backed by the generated settings catalog.
use super::*;
use Control::*;
pub use butterpollo_config::config::console_catalog::{
    APP, CLIENT, Control, GLOBAL, Setting, choice_label,
};

/// The label this page shows: the catalog's, except that the co-op component is named by what it
/// does (no third-party name in the console, GPL-3.0 section 7(e)). The key, and so the setting,
/// stays the same.
fn shown(setting: &Setting) -> &'static str {
    match setting.key {
        "background_stream" | "background-stream" => {
            "Stream in the background (co-op splitter, controllers only)"
        }
        _ => setting.label,
    }
}

pub fn render(prefix: &str, schema: &[Setting], values: &Value) -> String {
    let mut output = String::new();
    let mut previous = "";
    for setting in schema {
        if setting.group != previous {
            if !previous.is_empty() {
                output += "</div></details>";
            }
            output += &format!(
                "<details><summary>{}</summary><div class=\"settings-grid\">",
                i18n::message(setting.group)
            );
            previous = setting.group;
        }
        let name = format!("{prefix}{}", setting.key);
        let value = values
            .get(setting.key)
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| raw_value(value))
            })
            .unwrap_or_default();
        output += &match setting.control {
            Bool | Choice(_) => {
                let choices = match setting.control {
                    Bool => &["true", "false"][..],
                    Choice(choices) => choices,
                    _ => unreachable!(),
                };
                let mut options = vec![("", "Use default")];
                options.extend(choices.iter().map(|&choice| {
                    let label = choice_label(setting.key, choice).unwrap_or_else(|| {
                        if setting.key == "locale" {
                            i18n::language_name(choice)
                        } else {
                            choice
                        }
                    });
                    (choice, label)
                }));
                if !value.is_empty() && !choices.contains(&value.as_str()) {
                    options.push((&value, "Saved custom choice"));
                }
                select(&name, shown(setting), &value, &options)
            }
            Json => area(&name, shown(setting), &value),
            Number | Text => field(&name, shown(setting), &value, "text"),
        };
    }
    if !previous.is_empty() {
        output += "</div></details>";
    }
    output
}

fn raw_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

pub fn apply(
    prefix: &str,
    schema: &[Setting],
    fields: &Fields,
    values: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    for setting in schema {
        if let Some(value) = fields.get(&format!("{prefix}{}", setting.key)) {
            let value = value.trim();
            if value.is_empty() {
                values.insert(setting.key.into(), Value::Null);
            } else if matches!(setting.control, Json) {
                values.insert(
                    setting.key.into(),
                    serde_json::from_str(value)
                        .map_err(|error| format!("{}: {error}", setting.label))?,
                );
            } else {
                values.insert(setting.key.into(), Value::String(value.into()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_label_or_group_says_experimental() {
        let labelled: Vec<&str> = [GLOBAL, APP, CLIENT]
            .into_iter()
            .flatten()
            .flat_map(|setting| [setting.label, setting.group])
            .chain(super::super::PERMISSIONS.iter().map(|(_, label)| *label))
            .filter(|text| text.to_lowercase().contains("experiment"))
            .collect();
        assert!(labelled.is_empty(), "{labelled:?}");
    }
    #[test]
    fn no_shown_label_or_group_names_a_mod() {
        let named: Vec<&str> = [GLOBAL, APP, CLIENT]
            .into_iter()
            .flatten()
            .flat_map(|setting| [shown(setting), setting.group])
            .filter(|text| {
                text.contains("Nucleus")
                    || text
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|word| word == "ANR")
            })
            .collect();
        assert!(named.is_empty(), "{named:?}");
        // The setting keeps its key, so saving still writes it.
        let mut fields = Fields::new();
        fields.insert("client_background_stream".into(), "true".into());
        let mut values = serde_json::Map::new();
        apply("client_", CLIENT, &fields, &mut values).unwrap();
        assert_eq!(values["background_stream"], "true");
        let de = i18n::render(&render("client_", CLIENT, &Value::Null), "de");
        assert!(!de.contains("co-op splitter"), "untranslated: {de}");
    }
    #[test]
    fn hardware_metrics_settings_keep_their_typed_values() {
        let mut fields = Fields::new();
        fields.insert("cfg_hardware_metrics".into(), "true".into());
        fields.insert("cfg_hardware_metrics_interval".into(), "750".into());
        fields.insert(
            "cfg_hardware_metrics_selection".into(),
            r#"["cpu_usage_percent", "gpu_temperature_c"]"#.into(),
        );
        let mut values = serde_json::Map::new();
        apply("cfg_", GLOBAL, &fields, &mut values).unwrap();
        assert_eq!(values["hardware_metrics"], "true");
        assert_eq!(values["hardware_metrics_interval"], "750");
        assert_eq!(
            values["hardware_metrics_selection"],
            serde_json::json!(["cpu_usage_percent", "gpu_temperature_c"])
        );
        let html = render("cfg_", GLOBAL, &Value::Object(values));
        assert!(html.contains("Send hardware metrics to client"));
        assert!(html.contains("name=\"cfg_hardware_metrics_selection\""));
    }
    #[test]
    fn choice_labels_preserve_the_saved_values() {
        let html = i18n::render(&render("cfg_", GLOBAL, &serde_json::json!({})), "en");
        assert!(html.contains("value=\"vhf_ds5\">DualSense</option>"));
        assert!(html.contains("value=\"extended\">Extend — keep PC display primary</option>"));
        assert!(
            html.contains(
                "value=\"extended_primary\">Extend — make virtual display primary</option>"
            )
        );
        assert!(html.contains("value=\"enabled\">Four times the stream frame rate</option>"));
        assert!(!html.contains(">vhf_ds5</option>"));
    }
    #[test]
    fn stream_preset_is_a_choice_of_the_core_presets_in_two_languages() {
        let setting = GLOBAL
            .iter()
            .find(|setting| setting.key == "stream_preset")
            .unwrap();
        let Choice(choices) = setting.control else {
            panic!("a choice")
        };
        let ids: Vec<_> = butterpollo_media_policy::stream_preset::table()
            .presets
            .iter()
            .map(|preset| preset.id.as_str())
            .collect();
        assert_eq!(choices, ids);
        let values = serde_json::json!({"stream_preset": "reference"});
        let body = render("cfg_", GLOBAL, &values);
        let en = i18n::render(&body, "en");
        assert!(en.contains("value=\"reference\" selected>Reference quality</option>"));
        assert!(en.contains("value=\"lowest_latency\">Lowest latency</option>"));
        let de = i18n::render(&body, "de");
        assert!(
            de.contains("value=\"reference\" selected>Referenzqualität</option>"),
            "{de}"
        );
        assert!(de.contains("Stream-Voreinstellung"));
        // Saving writes the chosen name.
        let mut fields = Fields::new();
        fields.insert("cfg_stream_preset".into(), "reference".into());
        let mut saved = serde_json::Map::new();
        apply("cfg_", GLOBAL, &fields, &mut saved).unwrap();
        assert_eq!(saved["stream_preset"], "reference");
    }
    #[test]
    fn encoder_focus_offers_only_its_opt_in_values() {
        let setting = GLOBAL
            .iter()
            .find(|setting| setting.key == "encoder_focus")
            .unwrap();
        let Choice(choices) = setting.control else {
            panic!("a choice")
        };
        for choice in choices {
            assert!(
                butterpollo_media_policy::encoder_focus::Requested::parse(choice).is_ok(),
                "{choice}"
            );
        }
        assert!(!choices.contains(&"auto"));
        let html = i18n::render(
            &render(
                "cfg_",
                GLOBAL,
                &serde_json::json!({"encoder_focus": "static"}),
            ),
            "en",
        );
        assert!(html.contains("value=\"static\" selected>Still areas</option>"));
        assert!(html.contains("value=\"centre\">Still areas and centre</option>"));
    }
    #[test]
    fn the_nvidia_group_lists_every_setting_the_nvenc_encoder_reads() {
        // The encoder's settings, the GPU priority it runs at and the
        // driver's power profile.
        let source = [
            include_str!("../../../../core/media-policy/src/nvenc.rs"),
            include_str!("../../../../windows/graphics/src/gpu_priority.rs"),
            include_str!("../../../../windows/graphics/src/nvidia_profile.rs"),
        ]
        .concat();
        let keys: std::collections::BTreeSet<&str> = source
            .match_indices("\"nvenc_")
            .filter_map(|(at, _)| source[at + 1..].split('"').next())
            .filter(|key| {
                key.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .collect();
        let listed: std::collections::BTreeSet<&str> = GLOBAL
            .iter()
            .filter(|setting| setting.key.starts_with("nvenc_"))
            .map(|setting| setting.key)
            .collect();
        assert_eq!(listed, keys);
        let html = i18n::render(
            &render(
                "cfg_",
                GLOBAL,
                &serde_json::json!({"nvenc_split_encode": "three_strips", "nvenc_twopass": "full_res"}),
            ),
            "en",
        );
        assert!(html.contains("value=\"three_strips\" selected>Three strips</option>"));
        assert!(html.contains("value=\"full_res\" selected>Full resolution</option>"));
        assert!(html.contains("value=\"vbr\">Variable bitrate</option>"));
    }
    #[test]
    fn encoder_settings_save_keys_the_encoders_read() {
        for setting in GLOBAL {
            // The NVIDIA power profile is written once at startup for the
            // whole process, so it stays host-wide.
            if (setting.key.starts_with("amd_")
                || setting.key.starts_with("nvenc_")
                || setting.key == "encoder_focus")
                && setting.key != "nvenc_latency_over_power"
            {
                assert!(
                    butterpollo_config::config::override_allowed(setting.key),
                    "{} is not an encoder setting",
                    setting.key
                );
            }
        }
    }
    #[test]
    fn frame_warp_is_a_switch_a_device_may_override() {
        let setting = GLOBAL
            .iter()
            .find(|setting| setting.key == "frame_warp")
            .unwrap();
        assert!(matches!(setting.control, Bool));
        assert_eq!(setting.label, "Let devices use Frame Warp");
        assert!(butterpollo_config::config::override_allowed("frame_warp"));
        let mut fields = Fields::new();
        fields.insert("cfg_frame_warp".into(), "false".into());
        let mut values = serde_json::Map::new();
        apply("cfg_", GLOBAL, &fields, &mut values).unwrap();
        assert_eq!(values["frame_warp"], "false");
    }
}
