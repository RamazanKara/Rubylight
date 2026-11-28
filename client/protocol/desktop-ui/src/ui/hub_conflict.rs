//! Plain-English Hub Sync conflict resolver modal, status banner copy,
//! and host-override disclosure helper (`app × device -> app -> device -> host`).

use super::{HubConflictChoice, *};
use crate::hosts_model::HubStateView;
use nightjar_policy::hub_sync::{ProfileConflict, SyncStatus};
use rubylight_native_api::hub::HostOverrideSummary;

/// Draws the modal conflict resolver when a concurrent profile edit needs a choice.
pub(super) fn draw(
    ctx: &egui::Context,
    host: u64,
    conflict: &ProfileConflict,
    focus_first: bool,
    commands: &mut Vec<Command>,
) -> (bool, bool) {
    let mut close = false;
    let mut focused = false;
    modal(ctx, "hub-conflict", |ui| {
        ui.set_width(480.0);
        ui.label(
            RichText::new(tr("Profile changed on another device")).font(semibold(text::SECTION)),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(tr(
                "This profile changed on another device. Choose which version to keep.",
            ))
            .font(regular(text::BODY))
            .color(*ON_SURFACE_VARIANT),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(trf(
                "Profile: {} (PC revision {})",
                &[&conflict.remote.name, &conflict.server_revision],
            ))
            .font(semibold(text::BODY)),
        );
        if !conflict.conflicting_fields.is_empty() {
            let fields = format_fields(&conflict.conflicting_fields);
            ui.add_space(4.0);
            ui.label(
                RichText::new(trf("Changed in both places: {}", &[&fields]))
                    .font(regular(text::LABEL))
                    .color(*ON_SURFACE_VARIANT),
            );
        }
        ui.add_space(12.0);
        dialog_buttons(ui, |ui| {
            let keep = primary_button(ui, "Keep this device's changes");
            if focus_first {
                keep.request_focus();
                focused = true;
            }
            if keep.clicked() {
                commands.push(Command::ResolveHubConflict {
                    host,
                    choice: HubConflictChoice::KeepMine,
                });
                close = true;
            }
            if quiet_button(ui, "Use PC's version").clicked() {
                commands.push(Command::ResolveHubConflict {
                    host,
                    choice: HubConflictChoice::UseHost,
                });
                close = true;
            }
            if quiet_button(ui, "Save as copy").clicked() {
                commands.push(Command::ResolveHubConflict {
                    host,
                    choice: HubConflictChoice::SaveAsCopy,
                });
                close = true;
            }
            if quiet_button(ui, "Cancel").clicked() {
                close = true;
            }
        });
    });
    (close, focused)
}

/// Visual tone for [`sync_status_label`] banners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncTone {
    Warn,
    Muted,
}

/// Optional status banner text for offline outbox or permission errors.
pub fn sync_status_label(hub: &HubStateView) -> Option<(SyncTone, String)> {
    if hub.conflict.is_some() {
        return Some((
            SyncTone::Warn,
            tr("This profile changed on another device. Choose which version to keep.").into(),
        ));
    }
    match hub.status {
        SyncStatus::PermissionDenied => Some((
            SyncTone::Warn,
            tr("This device can view profiles from this PC, but needs Launch permission in the host console to save changes.").into(),
        )),
        SyncStatus::OfflineDegraded if hub.pending_outbox_count > 0 => Some((
            SyncTone::Muted,
            tr("PC is offline. Changes are saved on this device and will sync automatically when the PC reconnects.").into(),
        )),
        _ => None,
    }
}

/// Draws a read-only host-override notice under a game's rows when the PC forces settings.
pub(super) fn draw_override_notice(
    ui: &mut Ui,
    hub: &HubStateView,
    app_id: u32,
    left: f32,
    right: f32,
    mut y: f32,
) -> f32 {
    let active: Vec<&HostOverrideSummary> = hub
        .host_overrides
        .iter()
        .filter(|o| o.app_id == app_id || o.app_id == 0)
        .collect();
    if active.is_empty() {
        return y;
    }
    let width = (right - left).max(120.0);
    let keys: Vec<&str> = active
        .iter()
        .flat_map(|o| o.locked_keys.iter().map(String::as_str))
        .collect();
    let title = tr("Overridden on PC for this game");
    let detail = trf("PC override precedence applies ({}).", &[&keys.join(", ")]);
    let line1 = ui
        .painter()
        .layout(title.to_owned(), semibold(text::LABEL), *WARN, width);
    let line2 = ui
        .painter()
        .layout(detail, regular(text::CAPTION), *ON_SURFACE_VARIANT, width);
    ui.painter().galley(pos2(left, y), line1.clone(), *WARN);
    y += line1.size().y + 2.0;
    ui.painter()
        .galley(pos2(left, y), line2.clone(), *ON_SURFACE_VARIANT);
    y += line2.size().y + 10.0;
    y
}

fn format_fields(fields: &[String]) -> String {
    fields
        .iter()
        .map(|f| match f.as_str() {
            "name" => tr("Profile name").to_owned(),
            "scope" => tr("Sharing scope").to_owned(),
            "network_matcher" => tr("Network match").to_owned(),
            "input_deck_id" => tr("Controller layout").to_owned(),
            "stream.width" | "stream.height" => tr("Resolution").to_owned(),
            "stream.fps" => tr("Frame rate").to_owned(),
            "stream.bitrate_kbps" => tr("Bitrate").to_owned(),
            "stream.codec" => tr("Codec").to_owned(),
            "stream.hdr" => tr("HDR").to_owned(),
            "stream.upscaling" => tr("Upscaling").to_owned(),
            "stream.resolution_scale_pct" => tr("Resolution scale").to_owned(),
            "stream.display_matcher" => tr("Display").to_owned(),
            other => other.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{Lang, with_lang};
    use rubylight_native_api::hub::{ProfileScope, ProfileWrite, StreamProfileSpec, SyncProfile};

    fn sample_conflict() -> ProfileConflict {
        let remote = SyncProfile {
            id: "couch".into(),
            revision: 8,
            scope: ProfileScope::Device,
            name: "Living Room".into(),
            stream: StreamProfileSpec {
                fps: Some(90),
                ..Default::default()
            },
            ..Default::default()
        };
        ProfileConflict {
            id: "couch".into(),
            server_revision: 8,
            conflicting_fields: vec!["stream.fps".into()],
            base: None,
            local: ProfileWrite {
                scope: ProfileScope::Device,
                name: "Living Room".into(),
                stream: StreamProfileSpec {
                    fps: Some(120),
                    ..Default::default()
                },
                ..Default::default()
            },
            remote,
        }
    }

    #[test]
    fn conflict_dialog_and_override_notice_render_in_english_and_german() {
        let conflict = sample_conflict();
        for lang in [Lang::En, Lang::De] {
            with_lang(lang, || {
                let ctx = egui::Context::default();
                let mut commands = Vec::new();
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let _ = draw(ui.ctx(), 1, &conflict, true, &mut commands);
                    let hub = HubStateView {
                        host_overrides: vec![HostOverrideSummary {
                            app_id: 1042,
                            source: "app_device".into(),
                            locked_keys: vec!["max_bitrate".into(), "hdr".into()],
                        }],
                        ..Default::default()
                    };
                    let y = draw_override_notice(ui, &hub, 1042, 20.0, 420.0, 20.0);
                    assert!(y > 20.0);
                });
                out.textures_delta.clear();
                assert!(commands.is_empty());
                assert!(matches!(
                    sync_status_label(&HubStateView {
                        status: SyncStatus::PermissionDenied,
                        ..Default::default()
                    }),
                    Some((SyncTone::Warn, _))
                ));
                assert!(matches!(
                    sync_status_label(&HubStateView {
                        status: SyncStatus::OfflineDegraded,
                        pending_outbox_count: 1,
                        ..Default::default()
                    }),
                    Some((SyncTone::Muted, _))
                ));
            });
        }
    }
}
