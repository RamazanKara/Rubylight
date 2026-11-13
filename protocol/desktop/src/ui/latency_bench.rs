//! Settings → Advanced → Latency benchmark: this device's display, input, timer and decoder timing
//! without a PC, as Android's `LatencyBenchmarkActivity`. The test itself is
//! `crate::latency_bench`; app/launcher.rs starts it, feeds it the launcher's frames and input, and stops
//! it when the page is left.

use super::*;
use crate::latency_bench::{Codec, Metric};

/// A result row has its label and its value, no hint.
const NO_HINT: &str = "";

impl Launcher {
    pub(super) fn latency_benchmark_page(&mut self, ui: &mut Ui, _view: &View<'_>, commands: &mut Vec<Command>) {
        let focus = std::mem::take(&mut self.focus_screen);
        let bench = &self.latency_bench;
        // Every frame of the test is a sample: the page draws each refresh until it ends.
        if bench.timing() {
            ui.ctx().request_repaint();
        }
        let mut copy = false;
        column(ui, SETTINGS_W, |ui| {
            ui.horizontal(|ui| {
                let running = bench.running();
                let button = if running { primary_button(ui, "Stop test") } else { primary_button(ui, "Start 10-second test") };
                if focus {
                    button.request_focus();
                }
                if button.clicked() {
                    commands.push(if running { Command::StopLatencyBenchmark } else { Command::StartLatencyBenchmark });
                }
                copy = quiet_button(ui, "Copy results").clicked();
            });
            ui.add_space(space::L);
            let device = bench.device.clone().unwrap_or_default();
            let unknown = tr("no data yet");
            let caption = |ui: &mut Ui, line: String| {
                ui.label(RichText::new(line).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
            };
            if let Some(hz) = device.panel_hz {
                caption(ui, trf("Reported panel refresh: {} Hz", &[&format!("{hz:.2}")]));
            }
            let (rate, frames) = device.audio.map_or_else(|| (unknown.to_owned(), unknown.to_owned()), |(rate, frames)| (rate.to_string(), frames.to_string()));
            caption(ui, trf("Reported audio: {0} Hz · {1} frames per buffer", &[&rate, &frames]));
            ui.add_space(space::M);
            for metric in Metric::ALL {
                let value = bench.shown(metric).map_or_else(|| unknown.to_owned(), |summary| summary.text());
                row(ui, metric.label(), NO_HINT, |ui| ui.label(RichText::new(value).font(regular(text::BODY)).color(ON_SURFACE_VARIANT)));
            }
            section(ui, "Decoder benchmark");
            for codec in Codec::ALL {
                let value = match bench.result(codec) {
                    Some(result) => match &result.outcome {
                        Ok(measured) => {
                            let ms = trf("{} ms", &[&format!("{:.2}", measured.median_us as f64 / 1000.0)]);
                            format!("{ms} · {}", measured.hardware.unwrap_or(tr("Software")))
                        }
                        Err(_) => tr("Not supported").to_owned(),
                    },
                    None if bench.running() => tr("Testing…").to_owned(),
                    None => unknown.to_owned(),
                };
                row(ui, codec.name(), NO_HINT, |ui| ui.label(RichText::new(value).font(regular(text::BODY)).color(ON_SURFACE_VARIANT)));
            }
        });
        if copy {
            ui.ctx().copy_text(self.latency_bench.json(crate::VERSION));
            self.toast(tr("Results copied"));
        }
    }
}
