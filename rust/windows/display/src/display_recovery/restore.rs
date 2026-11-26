//! Journal replay and per-display restoration after a host crash or interrupted session.

use super::{Entry, Journal, Mode, dpi, hooks, put_back};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path};

/// Keep the original value until restoration succeeds (or the operation
/// establishes that the user has since changed it).
pub(super) fn restored<T>(pending: &mut Option<T>, result: Result<()>) -> Result<()> {
    result?;
    *pending = None;
    Ok(())
}

/// The same for a restore that covers every entry at once, such as the
/// frame limiter or the audio route.
pub(super) fn restored_all(
    kept: &mut Journal,
    stale_only: bool,
    flag: fn(&mut Entry) -> &mut bool,
    result: Result<()>,
) -> Result<()> {
    result?;
    for entry in kept.entries.values_mut() {
        if !stale_only || entry.stale {
            *flag(entry) = false;
        }
    }
    Ok(())
}

/// Undo what an interrupted host changed. Every step is attempted even when
/// an earlier one fails, and the failures are reported together. A display
/// that is not connected now, such as a TV that was switched off, keeps its
/// own HDR, colour profile, mode and position entries for a later recovery:
/// Windows remembers those settings per display, so they would otherwise
/// stay changed. Failed per-display changes and arrangement restoration are
/// kept for a later recovery as well.
pub(super) fn recover(path: &Path) -> Result<()> {
    // Switching to the active input desktop (Winlogon or Default) avoids
    // Access Denied when restoring display HDR or modes across lock transitions.
    crate::desktop::on_input_desktop(|| recover_inline(path))
}

fn recover_inline(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    let current_pid = std::process::id();
    let stale_only = journal.pid == current_pid;
    let directory = path.parent().context("journal directory missing")?;
    let mut failures = Vec::new();
    let mut step = |what: &str, result: Result<()>| {
        if let Err(error) = result {
            failures.push(format!("{what}: {error:#}"));
        }
    };
    let mut kept = Journal {
        pid: if stale_only { current_pid } else { 0 },
        started: if stale_only { journal.started } else { 0 },
        entries: journal
            .entries
            .iter()
            .map(|(id, entry)| {
                if stale_only && !entry.stale {
                    (id.clone(), entry.clone())
                } else {
                    let mut copied = entry.display_settings();
                    copied.stale = entry.stale;
                    (id.clone(), copied)
                }
            })
            .collect(),
    };
    if journal
        .entries
        .values()
        .any(|e| (!stale_only || e.stale) && e.external)
    {
        step(
            "frame limiter",
            restored_all(
                &mut kept,
                stale_only,
                |e| &mut e.external,
                hooks::limiter(directory),
            ),
        );
    }
    if journal
        .entries
        .values()
        .any(|e| (!stale_only || e.stale) && e.audio)
    {
        step(
            "audio routing",
            restored_all(
                &mut kept,
                stale_only,
                |e| &mut e.audio,
                hooks::audio(directory),
            ),
        );
    }
    let mut pending_arrangements = BTreeMap::new();
    for (id, entry) in journal
        .entries
        .iter()
        .filter(|(_, e)| !stale_only || e.stale)
    {
        if let Some((before, applied)) = &entry.arrangement {
            let result = crate::display_arrangement::restore_arrangement(before, applied);
            if result.is_err() {
                pending_arrangements.insert(id.clone(), (entry.arrangement.clone(), entry.stale));
            }
            step("display arrangement", result);
        }
    }
    let monitors = crate::display::monitors().unwrap_or_else(|error| {
        step("display list", Err(error));
        Vec::new()
    });
    // Scales put back below, checked once the layout is restored.
    let mut scales = BTreeMap::new();
    for (id, (arrangement, stale)) in pending_arrangements {
        let kept_entry = kept.entries.entry(id).or_default();
        kept_entry.arrangement = arrangement;
        kept_entry.stale = stale;
    }
    for entry in kept
        .entries
        .values_mut()
        .filter(|entry| (!stale_only || entry.stale) && entry.microphone)
    {
        let result = hooks::microphone(directory);
        // An unplugged original microphone must remain recoverable after a
        // later restart, just like a display that is temporarily unavailable.
        entry.microphone = result.is_err();
        step("microphone routing", result);
    }
    for (id, entry) in journal
        .entries
        .iter()
        .filter(|(_, e)| !stale_only || e.stale)
    {
        let Some(m) = monitors.iter().find(|m| &m.device_id == id) else {
            // Each setting is restored later only if the display still has
            // the value the stream applied.
            continue;
        };
        let pending = kept.entries.get_mut(id).expect("copied display entry");
        let what = |setting: &str| format!("{} {setting}", m.display_name);
        if let Some((previous, applied, system)) = &entry.profile {
            step(
                &what("colour profile"),
                restored(
                    &mut pending.profile,
                    crate::hdr_profile::restore(m, previous.as_deref(), applied, *system),
                ),
            );
        }
        if let Some((previous, applied)) = entry.hdr {
            let result = if crate::display::hdr_matches_applied(
                m.hdr_enabled,
                m.hdr_requested,
                applied,
            ) {
                tracing::info!(
                    output = %m.display_name,
                    previous,
                    applied,
                    current = m.hdr_enabled,
                    requested = m.hdr_requested,
                    "restoring HDR after a stream"
                );
                crate::display::set_hdr(m, previous)
            } else {
                tracing::info!(
                    output = %m.display_name,
                    previous,
                    applied,
                    current = m.hdr_enabled,
                    requested = m.hdr_requested,
                    "leaving HDR as it is; the display no longer has the value the stream applied"
                );
                Ok(())
            };
            step(&what("HDR"), restored(&mut pending.hdr, result));
        }
        // Before the mode: the mode applied was recorded in the shape the
        // display had before it turned.
        if let Some((previous, applied)) = entry.rotation {
            step(
                &what("rotation"),
                restored(
                    &mut pending.rotation,
                    put_back(
                        crate::display::rotation(id),
                        previous,
                        applied,
                        |previous| crate::display::set_rotation(id, previous),
                    ),
                ),
            );
        }
        if let Some((previous, applied)) = entry.mode {
            step(
                &what("mode"),
                restored(
                    &mut pending.mode,
                    crate::display::mode(&m.display_name).and_then(|current| {
                        if (
                            current.dmPelsWidth,
                            current.dmPelsHeight,
                            current.dmDisplayFrequency,
                        ) == applied
                        {
                            crate::display::set_mode(
                                &m.display_name,
                                previous.0,
                                previous.1,
                                previous.2,
                            )
                        } else {
                            Ok(())
                        }
                    }),
                ),
            );
        }
        if let Some((previous, applied)) = entry.mode_rate {
            step(
                &what("mode and refresh"),
                restored(
                    &mut pending.mode_rate,
                    (|| {
                        let current = crate::display::mode(&m.display_name)?;
                        let rate = crate::display::Topology::query()?.refresh(id)?;
                        let now = (current.dmPelsWidth, current.dmPelsHeight, rate.0);
                        // A stream that ended cleanly recorded the mode the
                        // display actually took, so it is put back only while
                        // the display still has it: a change the user made
                        // since stays. After a crash the recorded mode may be
                        // the one requested before the apply, which the display
                        // may have taken only in part: the original comes back
                        // while the display shows nothing but requested or
                        // original values.
                        if !restore_mode_needed(entry.mode_rate_requested, now, applied, previous) {
                            return Ok(());
                        }
                        crate::display::Topology::set_mode_rate(
                            id,
                            previous.0,
                            previous.1,
                            butterpollo_media_policy::framegen::Rate(previous.2),
                        )?;
                        Ok(())
                    })(),
                ),
            );
        }
        if let Some((previous, applied)) = entry.scale {
            step(
                &what("scale"),
                restored(
                    &mut pending.scale,
                    crate::display::dpi_scale(m).and_then(|current| {
                        if !restore_scale_needed(entry.scale_requested, current, applied, previous)
                        {
                            return Ok(());
                        }
                        crate::display::set_dpi_scale(m, previous)?;
                        scales.insert(id.clone(), previous);
                        Ok(())
                    }),
                ),
            );
        }
    }
    step(
        "display positions",
        (|| {
            let mut topology = crate::display::Topology::query()?;
            let nodes = topology.nodes()?;
            let positions = journal
                .entries
                .iter()
                .filter(|(_, entry)| !stale_only || entry.stale)
                .filter_map(|(id, entry)| {
                    let (before, applied) = entry.position?;
                    nodes
                        .iter()
                        .find(|n| &n.device_id == id && n.desired_position == applied)
                        .map(|_| (id.clone(), before))
                })
                .collect::<BTreeMap<_, _>>();
            if !positions.is_empty() {
                topology.set_positions(&positions)?;
            }
            for (id, pending) in &mut kept.entries {
                if (!stale_only || pending.stale) && nodes.iter().any(|node| &node.device_id == id)
                {
                    pending.position = None;
                }
            }
            Ok(())
        })(),
    );
    for (id, entry) in journal
        .entries
        .iter()
        .filter(|(_, e)| !stale_only || e.stale)
    {
        let pending = kept.entries.get_mut(id).expect("copied display entry");
        if let Some((before, applied)) = &entry.activation {
            step(
                "display activation",
                restored(
                    &mut pending.activation,
                    (|| {
                        if applied.is_empty() || crate::display_arrangement::unchanged(applied)? {
                            before.restore()?;
                        }
                        Ok(())
                    })(),
                ),
            );
        }
        if let Some((snapshot, excluded)) = &entry.baseline {
            step(
                "saved display layout",
                restored(&mut pending.baseline, snapshot.restore_excluding(excluded)),
            );
        }
    }
    // Last, as every step above can make Windows move a scale: the physical
    // baseline from before a virtual display, and the scales put back above.
    for entry in journal.entries.values().filter(|e| !stale_only || e.stale) {
        if let Some(baseline) = &entry.dpi_baseline {
            for (id, percent) in baseline {
                scales.entry(id.clone()).or_insert(*percent);
            }
        }
    }
    if !scales.is_empty() {
        let outcome = dpi::settle(&scales);
        for entry in kept.entries.values_mut().filter(|e| !stale_only || e.stale) {
            dpi::keep_drifted(&mut entry.dpi_baseline, &outcome);
        }
        if !outcome.drifted.is_empty() {
            step(
                "Windows scale",
                Err(anyhow::anyhow!(
                    "did not stay on {}",
                    outcome.drifted.join(", ")
                )),
            );
        }
    }
    // Keep a valid document so interrupted reads never see partial JSON. Non-stale
    // entries keep this host's PID/start token; un-recovered crash entries have
    // no live lease when `stale_only` is false.
    kept.entries.retain(|_, entry| entry.pending());
    if !stale_only {
        for entry in kept.entries.values_mut() {
            entry.stale = false;
        }
    }
    butterpollo_session::state::write_json(path, &kept)?;
    if !failures.is_empty() {
        bail!("display recovery incomplete: {}", failures.join("; "));
    }
    Ok(())
}

/// Whether the scale a stream left on a display is put back. A clean end
/// recorded the scale that landed, so it is put back only while the display
/// still has it. After a crash inside the apply (`requested`) the original
/// comes back whenever the display shows anything else: Windows may have
/// landed on a scale of its own.
pub(super) fn restore_scale_needed(
    requested: bool,
    current: u32,
    applied: u32,
    previous: u32,
) -> bool {
    current != previous && (requested || current == applied)
}

/// Whether the mode a stream left on a display is put back. `requested`: the
/// journal recorded the mode requested before the apply, not the one that
/// landed. A clean end records the mode the display actually took, so it is
/// put back only while the display still has it. A crash inside the apply
/// leaves the requested mode, and the display may have taken only part of it
/// (a TV takes the resolution and keeps its old rate): every part of its mode
/// is then the requested or the original one, so the original comes back only
/// when the resolution and the refresh each are. Any other mode, such as one
/// the user set by hand after the crash, stays.
pub(super) fn restore_mode_needed(
    requested: bool,
    current: Mode,
    applied: Mode,
    previous: Mode,
) -> bool {
    let size = |mode: Mode| (mode.0, mode.1);
    current == applied
        || (requested
            && [applied, previous]
                .iter()
                .any(|m| size(*m) == size(current))
            && [applied, previous].iter().any(|m| m.2 == current.2))
}
