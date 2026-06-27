//! egui dashboard: pitch ribbon, formant scatter, weight gauge, level meter,
//! all rendered against shaded target zones.
//!
//! UX note (build-plan principle #5 + Phase 5): zones are *bands the user
//! moves into*, never "higher is better". Colors are calm — no alarm reds —
//! because this is used by people who may be anxious about their voice.

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, RichText, Vec2b};
use egui_plot::{Line, Plot, PlotBounds, PlotPoints, Points, Polygon};
use rtrb::Consumer;

use crate::audio::{AudioControls, AudioEngine};
use crate::types::VoiceFrame;

/// Configurable target ranges. These are population *starting points*, clearly
/// labeled as such in the UI — they are not goals and must not imply that
/// pushing any metric higher is "better".
#[derive(Clone, Copy)]
pub struct Targets {
    pub pitch_lo: f32,
    pub pitch_hi: f32,
    pub f1_lo: f32,
    pub f1_hi: f32,
    pub f2_lo: f32,
    pub f2_hi: f32,
    pub weight_lo: f32,
    pub weight_hi: f32,
}

impl Default for Targets {
    fn default() -> Self {
        Self {
            // Comfortable feminine baseline *band* — capped, not a floor.
            pitch_lo: 165.0,
            pitch_hi: 220.0,
            // Formant target region (provisional placeholders — validate!).
            f1_lo: 350.0,
            f1_hi: 850.0,
            f2_lo: 1700.0,
            f2_hi: 2600.0,
            // Vocal weight (H1-H2, dB) band — lighter weight = larger H1-H2.
            weight_lo: 3.0,
            weight_hi: 14.0,
        }
    }
}

const HISTORY_MS: u64 = 10_000;

// Calm palette.
const INK: Color32 = Color32::from_rgb(60, 70, 90);
const ZONE_FILL: Color32 = Color32::from_rgba_premultiplied(120, 180, 150, 40);
const ZONE_LINE: Color32 = Color32::from_rgb(120, 180, 150);
const TRACE: Color32 = Color32::from_rgb(95, 130, 200);

const FIXED: Vec2b = Vec2b { x: false, y: false };

pub struct VoiceApp {
    frames: VecDeque<VoiceFrame>,
    consumer: Option<Consumer<VoiceFrame>>,
    _engine: Option<AudioEngine>,
    controls: Option<Arc<AudioControls>>,
    status: String,
    targets: Targets,
    gain: f32,
    threshold: f32,
}

impl VoiceApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Calm light theme — see UX note above.
        cc.egui_ctx.set_visuals(egui::Visuals::light());

        let (engine, consumer, controls, status) = match crate::audio::start() {
            Ok((eng, cons)) => {
                let s = format!("Listening · {} @ {} Hz", eng.device_name, eng.sample_rate);
                let controls = eng.controls.clone();
                (Some(eng), Some(cons), Some(controls), s)
            }
            Err(e) => (None, None, None, format!("Audio unavailable: {e}")),
        };
        Self {
            frames: VecDeque::new(),
            consumer,
            _engine: engine,
            controls,
            status,
            targets: Targets::default(),
            gain: 1.0,
            threshold: 0.01,
        }
    }

    /// Drain the ring buffer and evict frames older than the history window.
    fn pump(&mut self) {
        if let Some(c) = self.consumer.as_mut() {
            while let Ok(f) = c.pop() {
                self.frames.push_back(f);
            }
        }
        if let Some(&VoiceFrame { timestamp_ms: now, .. }) = self.frames.back() {
            let cutoff = now.saturating_sub(HISTORY_MS);
            while let Some(front) = self.frames.front() {
                if front.timestamp_ms < cutoff {
                    self.frames.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    fn latest_voiced(&self) -> Option<VoiceFrame> {
        self.frames.iter().rev().find(|f| f.f0.is_some()).copied()
    }
}

impl eframe::App for VoiceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        ui.ctx().request_repaint(); // keep redrawing for live scrolling

        ui.add_space(4.0);
        ui.label(RichText::new("Voice Trainer").size(20.0).strong().color(INK));
        ui.label(RichText::new(&self.status).color(INK));
        ui.separator();

        let now_ms = self.frames.back().map(|f| f.timestamp_ms).unwrap_or(0);
        let now_s = now_ms as f64 / 1000.0;
        let x_min = now_s - (HISTORY_MS as f64 / 1000.0);

        self.pitch_ribbon(ui, x_min, now_s);
        ui.add_space(8.0);

        ui.columns(2, |cols| {
            self.formant_scatter(&mut cols[0]);
            self.right_column(&mut cols[1]);
        });

        ui.add_space(8.0);
        self.mic_controls(ui);
    }
}

impl VoiceApp {
    /// Mic boost + silence-threshold sliders, pushed live to the worker.
    fn mic_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Mic boost").color(INK));
            if ui
                .add(egui::Slider::new(&mut self.gain, 1.0..=30.0).suffix("×"))
                .changed()
            {
                if let Some(c) = &self.controls {
                    c.set_gain(self.gain);
                }
            }
            ui.add_space(16.0);
            ui.label(RichText::new("Silence threshold").color(INK));
            if ui
                .add(egui::Slider::new(&mut self.threshold, 0.0..=0.05).fixed_decimals(3))
                .changed()
            {
                if let Some(c) = &self.controls {
                    c.set_silence_rms(self.threshold);
                }
            }
        });
    }
}

impl VoiceApp {
    fn pitch_ribbon(&self, ui: &mut egui::Ui, x_min: f64, x_max: f64) {
        ui.label(RichText::new("Pitch (F0)").strong().color(INK));
        let t = self.targets;
        // Build continuous segments, breaking at unvoiced gaps.
        let mut segments: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut cur: Vec<[f64; 2]> = Vec::new();
        for f in &self.frames {
            match f.f0 {
                Some(hz) => cur.push([f.timestamp_ms as f64 / 1000.0, hz as f64]),
                None => {
                    if !cur.is_empty() {
                        segments.push(std::mem::take(&mut cur));
                    }
                }
            }
        }
        if !cur.is_empty() {
            segments.push(cur);
        }

        Plot::new("pitch")
            .height(220.0)
            .show_x(false)
            .auto_bounds(FIXED)
            .show(ui, |pui| {
                pui.set_plot_bounds(PlotBounds::from_min_max([x_min, 80.0], [x_max, 350.0]));
                pui.polygon(zone_box(t.pitch_lo, t.pitch_hi, x_min, x_max, true));
                for (i, seg) in segments.into_iter().enumerate() {
                    pui.line(
                        Line::new(format!("f0-{i}"), PlotPoints::from(seg))
                            .color(TRACE)
                            .width(2.0),
                    );
                }
            });
    }

    fn formant_scatter(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Formants (F1 × F2)").strong().color(INK));
        let t = self.targets;
        let points: Vec<[f64; 2]> = self
            .frames
            .iter()
            .filter_map(|f| match (f.f1, f.f2) {
                (Some(a), Some(b)) => Some([a as f64, b as f64]),
                _ => None,
            })
            .collect();
        let newest = points.last().copied();

        Plot::new("formants").height(260.0).show(ui, |pui| {
            pui.polygon(
                Polygon::new(
                    "target",
                    PlotPoints::from(vec![
                        [t.f1_lo as f64, t.f2_lo as f64],
                        [t.f1_hi as f64, t.f2_lo as f64],
                        [t.f1_hi as f64, t.f2_hi as f64],
                        [t.f1_lo as f64, t.f2_hi as f64],
                    ]),
                )
                .fill_color(ZONE_FILL)
                .stroke(egui::Stroke::new(1.0, ZONE_LINE)),
            );
            pui.points(
                Points::new("cloud", PlotPoints::from(points))
                    .radius(2.5)
                    .color(TRACE.gamma_multiply(0.5)),
            );
            if let Some(p) = newest {
                pui.points(
                    Points::new("now", PlotPoints::from(vec![p]))
                        .radius(6.0)
                        .color(TRACE),
                );
            }
        });
    }

    fn right_column(&self, ui: &mut egui::Ui) {
        let latest = self.latest_voiced();
        let level = self.frames.back().map(|f| f.rms).unwrap_or(0.0);
        let t = self.targets;
        let weight = latest.and_then(|f| f.weight);

        ui.label(RichText::new("Vocal weight (H1–H2)").strong().color(INK));
        weight_gauge(ui, weight, t.weight_lo, t.weight_hi);

        ui.add_space(16.0);
        ui.label(RichText::new("Input level").strong().color(INK));
        // RMS is small; scale for a readable meter.
        ui.add(egui::ProgressBar::new((level * 12.0).clamp(0.0, 1.0)).desired_width(220.0));

        ui.add_space(16.0);
        // Glanceable in-band readout — no raw numbers required.
        in_band_row(ui, "Pitch", latest.and_then(|f| f.f0), t.pitch_lo, t.pitch_hi);
        in_band_row(ui, "Weight", weight, t.weight_lo, t.weight_hi);

        ui.add_space(12.0);
        ui.label(
            RichText::new("Target bands are population starting points, not goals.")
                .italics()
                .size(11.0)
                .color(INK),
        );
    }
}

/// Build a shaded target band as a plot polygon. If `vertical_band` is true the
/// band spans the full x range between two y values (pitch ribbon); otherwise
/// it is a value band along x between 0..1 in y (the weight gauge).
fn zone_box(lo: f32, hi: f32, x_min: f64, x_max: f64, vertical_band: bool) -> Polygon<'static> {
    let pts = if vertical_band {
        vec![
            [x_min, lo as f64],
            [x_max, lo as f64],
            [x_max, hi as f64],
            [x_min, hi as f64],
        ]
    } else {
        vec![
            [lo as f64, 0.0],
            [hi as f64, 0.0],
            [hi as f64, 1.0],
            [lo as f64, 1.0],
        ]
    };
    Polygon::new("zone", PlotPoints::from(pts))
        .fill_color(ZONE_FILL)
        .stroke(egui::Stroke::new(1.0, ZONE_LINE))
}

/// Horizontal weight gauge: a tiny plot with a shaded target band and a marker
/// line at the current H1–H2 value. (Plot-based to reuse the band styling.)
fn weight_gauge(ui: &mut egui::Ui, value: Option<f32>, lo: f32, hi: f32) {
    let (dmin, dmax) = (-5.0f64, 22.0f64);
    Plot::new("weight")
        .height(56.0)
        .show_y(false)
        .show_grid(false)
        .auto_bounds(FIXED)
        .show(ui, |pui| {
            pui.set_plot_bounds(PlotBounds::from_min_max([dmin, 0.0], [dmax, 1.0]));
            pui.polygon(zone_box(lo, hi, dmin, dmax, false));
            if let Some(v) = value {
                pui.line(
                    Line::new("now", PlotPoints::from(vec![[v as f64, 0.0], [v as f64, 1.0]]))
                        .color(TRACE)
                        .width(3.0),
                );
            }
        });
}

/// One row showing whether a metric is currently inside its target band,
/// using a calm dot rather than alarm coloring or a raw number.
fn in_band_row(ui: &mut egui::Ui, label: &str, value: Option<f32>, lo: f32, hi: f32) {
    ui.horizontal(|ui| {
        let (color, text) = match value {
            Some(v) if v >= lo && v <= hi => (ZONE_LINE, "in band"),
            Some(_) => (Color32::from_rgb(210, 170, 110), "outside"),
            None => (Color32::from_gray(180), "—"),
        };
        let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 5.0, color);
        ui.label(RichText::new(format!("{label}: {text}")).color(INK));
    });
}
