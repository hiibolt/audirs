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
use serde::{Deserialize, Serialize};

use crate::audio::{AudioControls, AudioEngine};
use crate::settings::Settings;
use crate::types::VoiceFrame;

/// Configurable target ranges. These are population *starting points*, clearly
/// labeled as such in the UI — they are not goals and must not imply that
/// pushing any metric higher is "better".
#[derive(Clone, Copy, Serialize, Deserialize)]
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

/// How much frame history we retain (drives the 30s time-in-band window).
const HISTORY_MS: u64 = 30_000;
/// How much of that history the scrolling pitch ribbon shows.
const RIBBON_MS: u64 = 10_000;
/// Session-trend bucket size: one median-pitch point every this many ms.
const BUCKET_MS: u64 = 2_000;

// Palette: light-green target zones, soft-red out-of-zone, pink accents.
const INK: Color32 = Color32::from_rgb(70, 60, 72); // warm dark text
// Fills are pre-multiplied (alpha-blended values) so they can be `const`.
const ZONE_FILL: Color32 = Color32::from_rgba_premultiplied(56, 82, 61, 95); // light green
const ZONE_LINE: Color32 = Color32::from_rgb(95, 180, 120);
const OUT_FILL: Color32 = Color32::from_rgba_premultiplied(46, 29, 29, 50); // soft red
const OUT_LINE: Color32 = Color32::from_rgb(210, 110, 110);
const ACCENT: Color32 = Color32::from_rgb(235, 110, 175); // pink
const TRACE: Color32 = ACCENT;

const FIXED: Vec2b = Vec2b { x: false, y: false };

/// How strongly to smooth displayed values (0 = frozen, 1 = no smoothing).
/// Low, because the mic throws jittery outliers we want to ride over.
const SMOOTH_ALPHA: f32 = 0.18;

pub struct VoiceApp {
    frames: VecDeque<VoiceFrame>,
    consumer: Option<Consumer<VoiceFrame>>,
    engine: Option<AudioEngine>,
    controls: Option<Arc<AudioControls>>,
    status: String,
    settings: Settings,
    devices: Vec<String>,
    // Bucketed median pitch over the whole session: [seconds, Hz].
    session: Vec<[f64; 2]>,
    next_bucket_ms: u64,
    // Exponential-moving-average smoothing state per metric.
    sm_f0: Option<f32>,
    sm_f1: Option<f32>,
    sm_f2: Option<f32>,
    sm_weight: Option<f32>,
    sm_rms: f32,
}

impl VoiceApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Light);

        let settings = Settings::load();
        let mut app = Self {
            frames: VecDeque::new(),
            consumer: None,
            engine: None,
            controls: None,
            status: String::new(),
            settings,
            devices: crate::audio::list_input_devices(),
            session: Vec::new(),
            next_bucket_ms: BUCKET_MS,
            sm_f0: None,
            sm_f1: None,
            sm_f2: None,
            sm_weight: None,
            sm_rms: 0.0,
        };
        app.restart_audio();
        app
    }

    /// (Re)start capture using the currently-selected device, resetting all
    /// live state. Used at launch, on device change, and on reconnect.
    fn restart_audio(&mut self) {
        // Drop the old engine first so its stream/worker stop cleanly.
        self.engine = None;
        self.consumer = None;
        self.controls = None;
        self.frames.clear();
        self.session.clear();
        self.next_bucket_ms = BUCKET_MS;
        self.sm_f0 = None;
        self.sm_f1 = None;
        self.sm_f2 = None;
        self.sm_weight = None;
        self.sm_rms = 0.0;

        match crate::audio::start(self.settings.device.as_deref()) {
            Ok((eng, cons)) => {
                self.status = format!("Listening · {} @ {} Hz", eng.device_name, eng.sample_rate);
                let controls = eng.controls.clone();
                // Push persisted gain/threshold into the fresh worker.
                controls.set_gain(self.settings.gain);
                controls.set_silence_rms(self.settings.threshold);
                self.controls = Some(controls);
                self.consumer = Some(cons);
                self.engine = Some(eng);
            }
            Err(e) => {
                self.status = format!("Audio unavailable: {e}");
            }
        }
    }

    /// Drain the ring buffer, smooth each frame, and evict old frames.
    fn pump(&mut self) {
        // Take the consumer out to avoid borrowing self mutably twice.
        if let Some(mut c) = self.consumer.take() {
            while let Ok(raw) = c.pop() {
                let f = self.smooth_frame(raw);
                self.frames.push_back(f);
            }
            self.consumer = Some(c);
        }
        if let Some(&VoiceFrame { timestamp_ms: now, .. }) = self.frames.back() {
            // Aggregate session-trend buckets before evicting old frames.
            while now >= self.next_bucket_ms {
                let lo = self.next_bucket_ms.saturating_sub(BUCKET_MS);
                let hi = self.next_bucket_ms;
                let med = median(
                    self.frames
                        .iter()
                        .filter(|f| f.timestamp_ms >= lo && f.timestamp_ms < hi)
                        .filter_map(|f| f.f0),
                );
                if let Some(m) = med {
                    self.session.push([hi as f64 / 1000.0, m as f64]);
                }
                self.next_bucket_ms += BUCKET_MS;
            }

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

    /// Apply EMA smoothing to a raw frame so the displayed values ride over
    /// the mic's jittery outliers. State is kept across short unvoiced gaps so
    /// brief consonants don't reset the smoothing.
    fn smooth_frame(&mut self, raw: VoiceFrame) -> VoiceFrame {
        fn ema(state: &mut Option<f32>, v: Option<f32>) -> Option<f32> {
            match v {
                Some(x) => {
                    let n = match *state {
                        Some(p) => p + SMOOTH_ALPHA * (x - p),
                        None => x,
                    };
                    *state = Some(n);
                    Some(n)
                }
                None => None, // render a gap, but keep state for continuity
            }
        }
        self.sm_rms += SMOOTH_ALPHA * (raw.rms - self.sm_rms);
        VoiceFrame {
            timestamp_ms: raw.timestamp_ms,
            f0: ema(&mut self.sm_f0, raw.f0),
            f1: ema(&mut self.sm_f1, raw.f1),
            f2: ema(&mut self.sm_f2, raw.f2),
            weight: ema(&mut self.sm_weight, raw.weight),
            rms: self.sm_rms,
        }
    }

    fn latest_voiced(&self) -> Option<VoiceFrame> {
        self.frames.iter().rev().find(|f| f.f0.is_some()).copied()
    }

    /// Fraction of *measurable* frames in the last `window_ms` that were inside
    /// the band, per `pred` (which returns `None` when the metric is absent and
    /// `Some(in_band)` otherwise). `None` if nothing measurable in the window.
    fn frac<P>(&self, window_ms: u64, pred: P) -> Option<f32>
    where
        P: Fn(&VoiceFrame) -> Option<bool>,
    {
        let now = self.frames.back()?.timestamp_ms;
        let cutoff = now.saturating_sub(window_ms);
        let (mut total, mut inb) = (0u32, 0u32);
        for f in self.frames.iter().rev() {
            if f.timestamp_ms < cutoff {
                break;
            }
            if let Some(ok) = pred(f) {
                total += 1;
                inb += ok as u32;
            }
        }
        (total > 0).then(|| inb as f32 / total as f32)
    }
}

impl eframe::App for VoiceApp {
    /// The window's clear color. eframe's default is hardcoded dark (12,12,12)
    /// regardless of theme, and since we draw no full-window panel it shows
    /// through everywhere — so we must override it to a light tone.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_gray(248).to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        // Force light theme every frame: eframe resolves the system theme at
        // frame start, so a one-time set in new() gets overridden back to dark.
        ui.ctx().set_visuals(egui::Visuals::light());
        ui.ctx().request_repaint(); // keep redrawing for live scrolling

        self.top_bar(ui);

        let now_ms = self.frames.back().map(|f| f.timestamp_ms).unwrap_or(0);
        let now_s = now_ms as f64 / 1000.0;
        let x_min = now_s - (RIBBON_MS as f64 / 1000.0);

        self.pitch_ribbon(ui, x_min, now_s);
        ui.add_space(8.0);

        ui.columns(2, |cols| {
            self.formant_scatter(&mut cols[0]);
            self.right_column(&mut cols[1]);
        });

        ui.add_space(8.0);
        self.session_view(ui);

        ui.add_space(8.0);
        self.mic_controls(ui);
    }
}

impl VoiceApp {
    /// Title, status, device picker, and connection/reconnect banner.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Voice Trainer").size(20.0).strong().color(ACCENT));
        ui.label(RichText::new(&self.status).color(INK));

        let disconnected = self.engine.as_ref().map_or(true, |e| e.lost());
        // Clone state out so the egui closures don't borrow `self`.
        let devices = self.devices.clone();
        let current = self.settings.device.clone();
        let mut new_device: Option<Option<String>> = None;
        let mut do_reconnect = false;
        let mut rescan = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Input").color(INK));
            let sel = current.clone().unwrap_or_else(|| "System default".to_string());
            egui::ComboBox::from_id_salt("device")
                .selected_text(sel)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "System default").clicked() {
                        new_device = Some(None);
                    }
                    for d in &devices {
                        let is = current.as_deref() == Some(d.as_str());
                        if ui.selectable_label(is, d).clicked() {
                            new_device = Some(Some(d.clone()));
                        }
                    }
                });
            if ui.button("⟳").on_hover_text("Rescan devices").clicked() {
                rescan = true;
            }
            if disconnected {
                ui.label(RichText::new("⚠ disconnected").strong().color(OUT_LINE));
                if ui.button("Reconnect").clicked() {
                    do_reconnect = true;
                }
            }
        });

        if rescan {
            self.devices = crate::audio::list_input_devices();
        }
        if let Some(choice) = new_device {
            self.settings.device = choice;
            self.settings.save();
            do_reconnect = true;
        }
        if do_reconnect {
            self.restart_audio();
        }
        ui.separator();
    }

    /// Session trajectory: median pitch per bucket over the whole run.
    fn session_view(&self, ui: &mut egui::Ui) {
        if self.session.len() < 2 {
            return;
        }
        ui.label(RichText::new("Session (pitch trend)").strong().color(ACCENT));
        let t = self.settings.targets;
        let max_x = self.session.last().map(|p| p[0]).unwrap_or(10.0).max(10.0);
        Plot::new("session").height(120.0).auto_bounds(FIXED).show(ui, |pui| {
            pui.set_plot_bounds(PlotBounds::from_min_max([0.0, 80.0], [max_x, 350.0]));
            pui.polygon(rect_poly(0.0, max_x, 80.0, 350.0, OUT_FILL, OUT_LINE));
            pui.polygon(rect_poly(
                0.0,
                max_x,
                t.pitch_lo as f64,
                t.pitch_hi as f64,
                ZONE_FILL,
                ZONE_LINE,
            ));
            pui.line(
                Line::new("trend", PlotPoints::from(self.session.clone()))
                    .color(ACCENT)
                    .width(2.0),
            );
        });
    }

    /// Mic boost + silence-threshold sliders, pushed live and persisted.
    fn mic_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Mic boost").color(INK));
            if ui
                .add(egui::Slider::new(&mut self.settings.gain, 1.0..=30.0).suffix("×"))
                .changed()
            {
                if let Some(c) = &self.controls {
                    c.set_gain(self.settings.gain);
                }
                changed = true;
            }
            ui.add_space(16.0);
            ui.label(RichText::new("Silence threshold").color(INK));
            if ui
                .add(egui::Slider::new(&mut self.settings.threshold, 0.0..=0.05).fixed_decimals(3))
                .changed()
            {
                if let Some(c) = &self.controls {
                    c.set_silence_rms(self.settings.threshold);
                }
                changed = true;
            }
        });
        if changed {
            self.settings.save();
        }
    }
}

impl VoiceApp {
    fn pitch_ribbon(&self, ui: &mut egui::Ui, x_min: f64, x_max: f64) {
        ui.label(RichText::new("Pitch (F0)").strong().color(ACCENT));
        let t = self.settings.targets;
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
                // Soft-red everywhere, light-green target band on top.
                pui.polygon(rect_poly(x_min, x_max, 80.0, 350.0, OUT_FILL, OUT_LINE));
                pui.polygon(rect_poly(
                    x_min,
                    x_max,
                    t.pitch_lo as f64,
                    t.pitch_hi as f64,
                    ZONE_FILL,
                    ZONE_LINE,
                ));
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
        ui.label(RichText::new("Formants (F1 × F2)").strong().color(ACCENT));
        let t = self.settings.targets;
        let points: Vec<[f64; 2]> = self
            .frames
            .iter()
            .filter_map(|f| match (f.f1, f.f2) {
                (Some(a), Some(b)) => Some([a as f64, b as f64]),
                _ => None,
            })
            .collect();
        let newest = points.last().copied();

        // Fixed view so the red/green regions are stable as points move.
        let (xb, yb) = ((150.0, 1100.0), (1400.0, 3000.0));
        Plot::new("formants")
            .height(260.0)
            .auto_bounds(FIXED)
            .show(ui, |pui| {
                pui.set_plot_bounds(PlotBounds::from_min_max([xb.0, yb.0], [xb.1, yb.1]));
                pui.polygon(rect_poly(xb.0, xb.1, yb.0, yb.1, OUT_FILL, OUT_LINE));
                pui.polygon(rect_poly(
                    t.f1_lo as f64,
                    t.f1_hi as f64,
                    t.f2_lo as f64,
                    t.f2_hi as f64,
                    ZONE_FILL,
                    ZONE_LINE,
                ));
                pui.points(
                    Points::new("cloud", PlotPoints::from(points))
                        .radius(2.5)
                        .color(ACCENT.gamma_multiply(0.45)),
                );
                if let Some(p) = newest {
                    pui.points(
                        Points::new("now", PlotPoints::from(vec![p]))
                            .radius(6.0)
                            .color(ACCENT),
                    );
                }
            });
    }

    fn right_column(&self, ui: &mut egui::Ui) {
        let latest = self.latest_voiced();
        let level = self.frames.back().map(|f| f.rms).unwrap_or(0.0);
        let t = self.settings.targets;
        let weight = latest.and_then(|f| f.weight);

        ui.label(RichText::new("Vocal weight (H1–H2)").strong().color(ACCENT));
        weight_gauge(ui, weight, t.weight_lo, t.weight_hi);

        ui.add_space(16.0);
        ui.label(RichText::new("Input level").strong().color(ACCENT));
        // RMS is small; scale for a readable meter.
        ui.add(egui::ProgressBar::new((level * 12.0).clamp(0.0, 1.0)).desired_width(220.0));

        ui.add_space(16.0);
        ui.label(RichText::new("In band").strong().color(ACCENT));

        // Per-metric in-band predicates (None when the metric isn't measurable).
        let pitch_pred = |f: &VoiceFrame| f.f0.map(|v| v >= t.pitch_lo && v <= t.pitch_hi);
        let fmt_pred = |f: &VoiceFrame| match (f.f1, f.f2) {
            (Some(a), Some(b)) => {
                Some(a >= t.f1_lo && a <= t.f1_hi && b >= t.f2_lo && b <= t.f2_hi)
            }
            _ => None,
        };
        let wt_pred = |f: &VoiceFrame| f.weight.map(|v| v >= t.weight_lo && v <= t.weight_hi);

        // "Now" = the most recent measurable frame: 1.0 in band, 0.0 out, None absent.
        let now = |pred: &dyn Fn(&VoiceFrame) -> Option<bool>| -> Option<f32> {
            self.frames
                .iter()
                .rev()
                .find_map(pred)
                .map(|b| if b { 1.0 } else { 0.0 })
        };

        egui::Grid::new("inband").spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("").color(INK));
            for h in ["now", "5s", "30s"] {
                ui.label(RichText::new(h).color(INK));
            }
            ui.end_row();

            bar_row(
                ui,
                "Pitch",
                now(&pitch_pred),
                self.frac(5_000, &pitch_pred),
                self.frac(30_000, &pitch_pred),
            );
            bar_row(
                ui,
                "Formants",
                now(&fmt_pred),
                self.frac(5_000, &fmt_pred),
                self.frac(30_000, &fmt_pred),
            );
            bar_row(
                ui,
                "Weight",
                now(&wt_pred),
                self.frac(5_000, &wt_pred),
                self.frac(30_000, &wt_pred),
            );
        });

        ui.add_space(12.0);
        ui.label(
            RichText::new("Target bands are population starting points, not goals.")
                .italics()
                .size(11.0)
                .color(INK),
        );
    }
}

/// Median of an iterator of values, or `None` if empty.
fn median(it: impl Iterator<Item = f32>) -> Option<f32> {
    let mut v: Vec<f32> = it.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

/// A filled axis-aligned rectangle as a plot polygon.
fn rect_poly(x0: f64, x1: f64, y0: f64, y1: f64, fill: Color32, line: Color32) -> Polygon<'static> {
    Polygon::new(
        "",
        PlotPoints::from(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]),
    )
    .fill_color(fill)
    .stroke(egui::Stroke::new(1.0, line))
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
            pui.polygon(rect_poly(dmin, dmax, 0.0, 1.0, OUT_FILL, OUT_LINE));
            pui.polygon(rect_poly(lo as f64, hi as f64, 0.0, 1.0, ZONE_FILL, ZONE_LINE));
            if let Some(v) = value {
                pui.line(
                    Line::new("now", PlotPoints::from(vec![[v as f64, 0.0], [v as f64, 1.0]]))
                        .color(ACCENT)
                        .width(3.0),
                );
            }
        });
}

/// One Grid row: metric name + three red→green fill bars (now / 5s / 30s).
fn bar_row(
    ui: &mut egui::Ui,
    label: &str,
    now: Option<f32>,
    w5: Option<f32>,
    w30: Option<f32>,
) {
    ui.label(RichText::new(label).color(INK));
    bar_cell(ui, now);
    bar_cell(ui, w5);
    bar_cell(ui, w30);
    ui.end_row();
}

/// A vertical bar filled from the bottom by `frac` (0..1), colored red→green by
/// the same value, on a light track. `None` = nothing measurable (track only).
/// For the instant column `frac` is just 1.0 (in band) or 0.0 (out) → full/empty.
fn bar_cell(ui: &mut egui::Ui, frac: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 34.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(232)); // track
    if let Some(f) = frac {
        let f = f.clamp(0.0, 1.0);
        if f > 0.0 {
            let top = rect.bottom() - rect.height() * f;
            let filled = egui::Rect::from_min_max(egui::pos2(rect.left(), top), rect.max);
            p.rect_filled(filled, 2.0, lerp_red_green(f));
        }
    }
}

/// Lerp red (0.0) → green (1.0) for the fill bars.
fn lerp_red_green(f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    Color32::from_rgb((40.0 + (1.0 - f) * 200.0) as u8, (40.0 + f * 180.0) as u8, 60)
}
