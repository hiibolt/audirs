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
        Self::feminine()
    }
}

impl Targets {
    /// Comfortable feminine reference band.
    pub const fn feminine() -> Self {
        Self {
            pitch_lo: 165.0,
            pitch_hi: 220.0,
            f1_lo: 350.0,
            f1_hi: 850.0,
            f2_lo: 1700.0,
            f2_hi: 2600.0,
            weight_lo: 3.0,
            weight_hi: 14.0,
        }
    }

    /// Comfortable masculine reference band.
    pub const fn masculine() -> Self {
        Self {
            pitch_lo: 85.0,
            pitch_hi: 155.0,
            f1_lo: 300.0,
            f1_hi: 750.0,
            f2_lo: 1100.0,
            f2_hi: 1900.0,
            weight_lo: -2.0,
            weight_hi: 6.0,
        }
    }

    /// Linear blend between `from` and `to` band edges, by `t` in [0, 1].
    pub fn lerp(from: Self, to: Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            pitch_lo: l(from.pitch_lo, to.pitch_lo),
            pitch_hi: l(from.pitch_hi, to.pitch_hi),
            f1_lo: l(from.f1_lo, to.f1_lo),
            f1_hi: l(from.f1_hi, to.f1_hi),
            f2_lo: l(from.f2_lo, to.f2_lo),
            f2_hi: l(from.f2_hi, to.f2_hi),
            weight_lo: l(from.weight_lo, to.weight_lo),
            weight_hi: l(from.weight_hi, to.weight_hi),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
}

impl Gender {
    pub fn opposite(self) -> Self {
        match self {
            Self::Male => Self::Female,
            Self::Female => Self::Male,
        }
    }
    pub fn targets(self) -> Targets {
        match self {
            Self::Male => Targets::masculine(),
            Self::Female => Targets::feminine(),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Male => "Male",
            Self::Female => "Female",
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
// Starting-zone overlay: light blue, drawn under the goal zone for comparison.
const START_FILL: Color32 = Color32::from_rgba_premultiplied(60, 100, 140, 70);
const START_LINE: Color32 = Color32::from_rgb(110, 160, 210);
const ACCENT: Color32 = Color32::from_rgb(235, 110, 175); // pink
const TRACE: Color32 = ACCENT;

const FIXED: Vec2b = Vec2b { x: false, y: false };

/// How strongly to smooth displayed values (0 = frozen, 1 = no smoothing).
/// Low, because the mic throws jittery outliers we want to ride over.
const SMOOTH_ALPHA: f32 = 0.18;

/// Running in-band tally for one metric over the active session.
#[derive(Default, Clone, Copy)]
struct InBand {
    inb: u64,
    total: u64,
}

impl InBand {
    fn add(&mut self, measured: Option<bool>) {
        if let Some(b) = measured {
            self.total += 1;
            self.inb += b as u64;
        }
    }
    fn frac(&self) -> Option<f32> {
        (self.total > 0).then(|| self.inb as f32 / self.total as f32)
    }
}

/// Frozen totals produced when a session is stopped.
struct SessionSummary {
    duration_s: f32,
    median_pitch: Option<f32>,
    pitch: Option<f32>,
    fmt: Option<f32>,
    weight: Option<f32>,
}

pub struct VoiceApp {
    frames: VecDeque<VoiceFrame>,
    consumer: Option<Consumer<VoiceFrame>>,
    engine: Option<AudioEngine>,
    controls: Option<Arc<AudioControls>>,
    status: String,
    settings: Settings,
    devices: Vec<String>,
    // --- session (user-controlled via Start/Stop) ---
    session_active: bool,
    sess_pitch: InBand,
    sess_fmt: InBand,
    sess_weight: InBand,
    sess_start_ms: u64,
    sess_last_ms: u64,
    sess_next_bucket_ms: u64,
    /// Bucketed median pitch over the session: [seconds-since-start, Hz].
    trend: Vec<[f64; 2]>,
    summary: Option<SessionSummary>,
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
            session_active: false,
            sess_pitch: InBand::default(),
            sess_fmt: InBand::default(),
            sess_weight: InBand::default(),
            sess_start_ms: 0,
            sess_last_ms: 0,
            sess_next_bucket_ms: BUCKET_MS,
            trend: Vec::new(),
            summary: None,
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
        // A capture restart resets the clock, so any active session ends.
        self.session_active = false;
        self.trend.clear();
        self.summary = None;
        self.sess_pitch = InBand::default();
        self.sess_fmt = InBand::default();
        self.sess_weight = InBand::default();
        self.sess_start_ms = 0;
        self.sess_last_ms = 0;
        self.sess_next_bucket_ms = BUCKET_MS;
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

    /// Drain the ring buffer, smooth each frame, accumulate session stats, and
    /// evict old frames.
    fn pump(&mut self) {
        // Take the consumer out to avoid borrowing self mutably twice.
        if let Some(mut c) = self.consumer.take() {
            while let Ok(raw) = c.pop() {
                let f = self.smooth_frame(raw);
                if self.session_active {
                    self.accumulate(&f);
                }
                self.frames.push_back(f);
            }
            self.consumer = Some(c);
        }
        if let Some(&VoiceFrame { timestamp_ms: now, .. }) = self.frames.back() {
            // Aggregate session-trend buckets (only while a session runs).
            while self.session_active && now >= self.sess_next_bucket_ms {
                let lo = self.sess_next_bucket_ms.saturating_sub(BUCKET_MS);
                let hi = self.sess_next_bucket_ms;
                let med = median(
                    self.frames
                        .iter()
                        .filter(|f| f.timestamp_ms >= lo && f.timestamp_ms < hi)
                        .filter_map(|f| f.f0),
                );
                if let Some(m) = med {
                    let x = (hi.saturating_sub(self.sess_start_ms)) as f64 / 1000.0;
                    self.trend.push([x, m as f64]);
                }
                self.sess_next_bucket_ms += BUCKET_MS;
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

    /// Fold one frame into the running session tallies (in-band vs the
    /// currently-effective goal zone).
    fn accumulate(&mut self, f: &VoiceFrame) {
        let t = self.settings.effective_targets();
        self.sess_pitch.add(f.f0.map(|v| v >= t.pitch_lo && v <= t.pitch_hi));
        self.sess_fmt.add(match (f.f1, f.f2) {
            (Some(a), Some(b)) => {
                Some(a >= t.f1_lo && a <= t.f1_hi && b >= t.f2_lo && b <= t.f2_hi)
            }
            _ => None,
        });
        self.sess_weight
            .add(f.weight.map(|v| v >= t.weight_lo && v <= t.weight_hi));
        self.sess_last_ms = f.timestamp_ms;
    }

    fn start_session(&mut self) {
        self.session_active = true;
        self.summary = None;
        self.trend.clear();
        self.sess_pitch = InBand::default();
        self.sess_fmt = InBand::default();
        self.sess_weight = InBand::default();
        let now = self.frames.back().map(|f| f.timestamp_ms).unwrap_or(0);
        self.sess_start_ms = now;
        self.sess_last_ms = now;
        self.sess_next_bucket_ms = now + BUCKET_MS;
    }

    fn stop_session(&mut self) {
        self.session_active = false;
        // Generate totals only if the session actually captured something.
        if self.sess_pitch.total > 0 || !self.trend.is_empty() {
            self.summary = Some(SessionSummary {
                duration_s: self.sess_last_ms.saturating_sub(self.sess_start_ms) as f32 / 1000.0,
                median_pitch: median(self.trend.iter().map(|p| p[1] as f32)),
                pitch: self.sess_pitch.frac(),
                fmt: self.sess_fmt.frac(),
                weight: self.sess_weight.frac(),
            });
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

    /// Mean of `prog(f)` over the last `window_ms` of measurable frames.
    fn avg<P>(&self, window_ms: u64, prog: P) -> Option<f32>
    where
        P: Fn(&VoiceFrame) -> Option<f32>,
    {
        let now = self.frames.back()?.timestamp_ms;
        let cutoff = now.saturating_sub(window_ms);
        let (mut total, mut sum) = (0u32, 0.0f32);
        for f in self.frames.iter().rev() {
            if f.timestamp_ms < cutoff {
                break;
            }
            if let Some(p) = prog(f) {
                total += 1;
                sum += p;
            }
        }
        (total > 0).then(|| sum / total as f32)
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
    }
}

impl VoiceApp {
    /// All settings, grouped compactly at the top: title/status, device + mic +
    /// session controls on one wrapped row, then the goal/gender row.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Voice Trainer").size(18.0).strong().color(ACCENT));
            ui.label(RichText::new(&self.status).color(INK));
        });

        let disconnected = self.engine.as_ref().map_or(true, |e| e.lost());
        let active = self.session_active;
        // Clone/copy state out so the egui closures don't borrow `self`.
        let devices = self.devices.clone();
        let current = self.settings.device.clone();
        let mut gain = self.settings.gain;
        let mut thr = self.settings.threshold;
        let mut new_device: Option<Option<String>> = None;
        let mut do_reconnect = false;
        let mut rescan = false;
        let mut mic_changed = false;
        let mut toggle_session = false;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().slider_width = 100.0;

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

            ui.separator();
            ui.label(RichText::new("Boost").color(INK));
            if ui.add(egui::Slider::new(&mut gain, 1.0..=30.0).suffix("×")).changed() {
                mic_changed = true;
            }
            ui.label(RichText::new("Silence").color(INK));
            if ui
                .add(egui::Slider::new(&mut thr, 0.0..=0.05).fixed_decimals(3))
                .changed()
            {
                mic_changed = true;
            }

            ui.separator();
            let label = if active { "■ Stop session" } else { "▶ Start session" };
            if ui.button(RichText::new(label).strong()).clicked() {
                toggle_session = true;
            }
            if disconnected {
                ui.label(RichText::new("⚠ disconnected").strong().color(OUT_LINE));
                if ui.button("Reconnect").clicked() {
                    do_reconnect = true;
                }
            }
        });

        if mic_changed {
            self.settings.gain = gain;
            self.settings.threshold = thr;
            if let Some(c) = &self.controls {
                c.set_gain(gain);
                c.set_silence_rms(thr);
            }
            self.settings.save();
        }
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
        if toggle_session {
            if active {
                self.stop_session();
            } else {
                self.start_session();
            }
        }

        self.goal_controls(ui);
        ui.separator();
    }

    /// Target-gender select, goal-percent slider, and starting-zone overlay.
    fn goal_controls(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Target").color(INK));
            let cur = self.settings.target_gender;
            egui::ComboBox::from_id_salt("gender")
                .selected_text(cur.label())
                .show_ui(ui, |ui| {
                    for opt in [Gender::Female, Gender::Male] {
                        if ui
                            .selectable_label(cur == opt, opt.label())
                            .clicked()
                            && cur != opt
                        {
                            self.settings.target_gender = opt;
                            changed = true;
                        }
                    }
                });

            ui.add_space(12.0);
            ui.label(RichText::new("Goal").color(INK));
            if ui
                .add(
                    egui::Slider::new(&mut self.settings.goal_percent, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                        .custom_parser(|s| {
                            s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)
                        }),
                )
                .changed()
            {
                changed = true;
            }

            ui.add_space(12.0);
            if ui
                .checkbox(&mut self.settings.show_starting, "Show starting zone")
                .changed()
            {
                changed = true;
            }
        });
        if changed {
            self.settings.save();
        }
    }

    /// Session totals (after Stop) and the median-pitch trajectory.
    fn session_view(&self, ui: &mut egui::Ui) {
        // Frozen totals from the last completed session.
        if let Some(s) = &self.summary {
            ui.separator();
            ui.label(RichText::new("Last session").strong().color(ACCENT));
            ui.label(
                RichText::new(format!(
                    "Duration {:.0}s · median pitch {} · in band — pitch {}, formants {}, weight {}",
                    s.duration_s,
                    opt_hz(s.median_pitch),
                    pct(s.pitch),
                    pct(s.fmt),
                    pct(s.weight),
                ))
                .color(INK),
            );
        }

        if self.trend.len() < 2 {
            return;
        }
        let title = if self.session_active {
            "Session (pitch trend, live)"
        } else {
            "Session (pitch trend)"
        };
        ui.label(RichText::new(title).strong().color(ACCENT));
        let t = self.settings.effective_targets();
        let start = self.settings.starting_targets();
        let show_start = self.settings.show_starting;
        let max_x = self.trend.last().map(|p| p[0]).unwrap_or(10.0).max(10.0);
        Plot::new("session").height(120.0).auto_bounds(FIXED).show(ui, |pui| {
            pui.set_plot_bounds(PlotBounds::from_min_max([0.0, 80.0], [max_x, 350.0]));
            pui.polygon(rect_poly(0.0, max_x, 80.0, 350.0, OUT_FILL, OUT_LINE));
            if show_start {
                pui.polygon(rect_poly(
                    0.0,
                    max_x,
                    start.pitch_lo as f64,
                    start.pitch_hi as f64,
                    START_FILL,
                    START_LINE,
                ));
            }
            pui.polygon(rect_poly(
                0.0,
                max_x,
                t.pitch_lo as f64,
                t.pitch_hi as f64,
                ZONE_FILL,
                ZONE_LINE,
            ));
            pui.line(
                Line::new("trend", PlotPoints::from(self.trend.clone()))
                    .color(ACCENT)
                    .width(2.0),
            );
        });
    }

}

impl VoiceApp {
    fn pitch_ribbon(&self, ui: &mut egui::Ui, x_min: f64, x_max: f64) {
        ui.label(RichText::new("Pitch (F0)").strong().color(ACCENT));
        let t = self.settings.effective_targets();
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
                if self.settings.show_starting {
                    let start = self.settings.starting_targets();
                    pui.polygon(rect_poly(
                        x_min,
                        x_max,
                        start.pitch_lo as f64,
                        start.pitch_hi as f64,
                        START_FILL,
                        START_LINE,
                    ));
                }
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
        let t = self.settings.effective_targets();
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
                if self.settings.show_starting {
                    let start = self.settings.starting_targets();
                    pui.polygon(rect_poly(
                        start.f1_lo as f64,
                        start.f1_hi as f64,
                        start.f2_lo as f64,
                        start.f2_hi as f64,
                        START_FILL,
                        START_LINE,
                    ));
                }
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
        let t = self.settings.effective_targets();
        let weight = latest.and_then(|f| f.weight);

        ui.label(RichText::new("Vocal weight (H1–H2)").strong().color(ACCENT));
        let start_band = self
            .settings
            .show_starting
            .then(|| {
                let s = self.settings.starting_targets();
                (s.weight_lo, s.weight_hi)
            });
        weight_gauge(ui, weight, t.weight_lo, t.weight_hi, start_band);

        ui.add_space(16.0);
        ui.label(RichText::new("Input level").strong().color(ACCENT));
        // RMS is small; scale for a readable meter.
        ui.add(egui::ProgressBar::new((level * 12.0).clamp(0.0, 1.0)).desired_width(220.0));

        ui.add_space(16.0);
        ui.label(RichText::new("In band").strong().color(ACCENT));

        if self.settings.show_starting {
            self.inband_bipolar(ui, t);
        } else {
            self.inband_unipolar(ui, t);
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new("Target bands are population starting points, not goals.")
                .italics()
                .size(11.0)
                .color(INK),
        );
    }

    /// In-band grid as fraction-in-band bars (no starting-zone overlay).
    fn inband_unipolar(&self, ui: &mut egui::Ui, t: Targets) {
        let pitch_pred = |f: &VoiceFrame| f.f0.map(|v| v >= t.pitch_lo && v <= t.pitch_hi);
        let fmt_pred = |f: &VoiceFrame| match (f.f1, f.f2) {
            (Some(a), Some(b)) => {
                Some(a >= t.f1_lo && a <= t.f1_hi && b >= t.f2_lo && b <= t.f2_hi)
            }
            _ => None,
        };
        let wt_pred = |f: &VoiceFrame| f.weight.map(|v| v >= t.weight_lo && v <= t.weight_hi);

        let now = |pred: &dyn Fn(&VoiceFrame) -> Option<bool>| -> Option<f32> {
            self.frames
                .iter()
                .rev()
                .find_map(pred)
                .map(|b| if b { 1.0 } else { 0.0 })
        };

        egui::Grid::new("inband").spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("").color(INK));
            for h in ["now", "5s", "30s", "session"] {
                ui.label(RichText::new(h).color(INK));
            }
            ui.end_row();

            bar_row(
                ui,
                "Pitch",
                now(&pitch_pred),
                self.frac(5_000, &pitch_pred),
                self.frac(30_000, &pitch_pred),
                self.sess_pitch.frac(),
            );
            bar_row(
                ui,
                "Formants",
                now(&fmt_pred),
                self.frac(5_000, &fmt_pred),
                self.frac(30_000, &fmt_pred),
                self.sess_fmt.frac(),
            );
            bar_row(
                ui,
                "Weight",
                now(&wt_pred),
                self.frac(5_000, &wt_pred),
                self.frac(30_000, &wt_pred),
                self.sess_weight.frac(),
            );
        });
    }

    /// In-band grid as bipolar gauges: bottom = deep in starting zone (blue),
    /// middle stroke = halfway, top = at/past the goal zone (green).
    fn inband_bipolar(&self, ui: &mut egui::Ui, t: Targets) {
        let s = self.settings.starting_targets();

        let pitch_prog =
            |f: &VoiceFrame| f.f0.map(|v| progress_1d(v, s.pitch_lo, s.pitch_hi, t.pitch_lo, t.pitch_hi));
        let fmt_prog = |f: &VoiceFrame| match (f.f1, f.f2) {
            (Some(a), Some(b)) => Some(progress_2d(a, b, &s, &t)),
            _ => None,
        };
        let wt_prog = |f: &VoiceFrame| {
            f.weight
                .map(|v| progress_1d(v, s.weight_lo, s.weight_hi, t.weight_lo, t.weight_hi))
        };

        let now = |prog: &dyn Fn(&VoiceFrame) -> Option<f32>| -> Option<f32> {
            self.frames.iter().rev().find_map(prog)
        };

        egui::Grid::new("inband").spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("").color(INK));
            for h in ["now", "5s", "30s", "session"] {
                ui.label(RichText::new(h).color(INK));
            }
            ui.end_row();

            bipolar_row(
                ui,
                "Pitch",
                now(&pitch_prog),
                self.avg(5_000, &pitch_prog),
                self.avg(30_000, &pitch_prog),
                self.sess_pitch.frac(),
            );
            bipolar_row(
                ui,
                "Formants",
                now(&fmt_prog),
                self.avg(5_000, &fmt_prog),
                self.avg(30_000, &fmt_prog),
                self.sess_fmt.frac(),
            );
            bipolar_row(
                ui,
                "Weight",
                now(&wt_prog),
                self.avg(5_000, &wt_prog),
                self.avg(30_000, &wt_prog),
                self.sess_weight.frac(),
            );
        });
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
fn weight_gauge(
    ui: &mut egui::Ui,
    value: Option<f32>,
    lo: f32,
    hi: f32,
    starting: Option<(f32, f32)>,
) {
    let (dmin, dmax) = (-5.0f64, 22.0f64);
    Plot::new("weight")
        .height(56.0)
        .show_y(false)
        .show_grid(false)
        .auto_bounds(FIXED)
        .show(ui, |pui| {
            pui.set_plot_bounds(PlotBounds::from_min_max([dmin, 0.0], [dmax, 1.0]));
            pui.polygon(rect_poly(dmin, dmax, 0.0, 1.0, OUT_FILL, OUT_LINE));
            if let Some((slo, shi)) = starting {
                pui.polygon(rect_poly(
                    slo as f64,
                    shi as f64,
                    0.0,
                    1.0,
                    START_FILL,
                    START_LINE,
                ));
            }
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
    sess: Option<f32>,
) {
    ui.label(RichText::new(label).color(INK));
    bar_cell(ui, now);
    bar_cell(ui, w5);
    bar_cell(ui, w30);
    bar_cell(ui, sess);
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

/// Format an optional Hz value for the session summary.
fn opt_hz(v: Option<f32>) -> String {
    match v {
        Some(x) => format!("{x:.0} Hz"),
        None => "—".to_string(),
    }
}

/// Format an optional fraction as a percentage for the session summary.
fn pct(v: Option<f32>) -> String {
    match v {
        Some(x) => format!("{:.0}%", x * 100.0),
        None => "—".to_string(),
    }
}

/// Lerp red (0.0) → green (1.0) for the fill bars.
fn lerp_red_green(f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    Color32::from_rgb((40.0 + (1.0 - f) * 200.0) as u8, (40.0 + f * 180.0) as u8, 60)
}

/// Map a 1D value `v` to a bipolar progress in [-1, 1]: -1 at the starting
/// band's center, +1 at the goal band's center, linearly interpolated.
fn progress_1d(v: f32, s_lo: f32, s_hi: f32, g_lo: f32, g_hi: f32) -> f32 {
    let s_c = (s_lo + s_hi) * 0.5;
    let g_c = (g_lo + g_hi) * 0.5;
    let denom = g_c - s_c;
    if denom.abs() < 1e-6 {
        return 0.0;
    }
    let t = (v - s_c) / denom;
    (2.0 * t - 1.0).clamp(-1.0, 1.0)
}

/// 2D version: project (v1, v2) onto the axis from the starting-zone center to
/// the goal-zone center, then map to [-1, 1] the same way.
fn progress_2d(v1: f32, v2: f32, s: &Targets, g: &Targets) -> f32 {
    let s1 = (s.f1_lo + s.f1_hi) * 0.5;
    let s2 = (s.f2_lo + s.f2_hi) * 0.5;
    let g1 = (g.f1_lo + g.f1_hi) * 0.5;
    let g2 = (g.f2_lo + g.f2_hi) * 0.5;
    let dx = g1 - s1;
    let dy = g2 - s2;
    let denom = dx * dx + dy * dy;
    if denom < 1e-6 {
        return 0.0;
    }
    let t = ((v1 - s1) * dx + (v2 - s2) * dy) / denom;
    (2.0 * t - 1.0).clamp(-1.0, 1.0)
}

/// One Grid row of bipolar gauges (now / 5s / 30s).
fn bipolar_row(
    ui: &mut egui::Ui,
    label: &str,
    now: Option<f32>,
    w5: Option<f32>,
    w30: Option<f32>,
    sess: Option<f32>,
) {
    ui.label(RichText::new(label).color(INK));
    bipolar_cell(ui, now);
    bipolar_cell(ui, w5);
    bipolar_cell(ui, w30);
    // Session column is a plain fraction-in-band bar (a "total"), even in
    // bipolar mode, so it reads as overall progress this session.
    bar_cell(ui, sess);
    ui.end_row();
}

/// Bipolar gauge cell: middle stroke is the midpoint between the starting and
/// goal zones; the cursor sits below it (toward a blue bottom) when the value
/// reads as the starting zone, and above it (toward a green top) when it nears
/// or exceeds the goal zone. Cursor is always drawn when a value is present.
fn bipolar_cell(ui: &mut egui::Ui, progress: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 34.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(232)); // track
    let mid_y = rect.center().y;
    // Center reference line.
    p.line_segment(
        [egui::pos2(rect.left(), mid_y), egui::pos2(rect.right(), mid_y)],
        egui::Stroke::new(1.0, Color32::from_gray(150)),
    );
    if let Some(prog) = progress {
        let pc = prog.clamp(-1.0, 1.0);
        let half = rect.height() * 0.5;
        let cursor_y = mid_y - pc * half;
        let (top, bot, color) = if pc >= 0.0 {
            (cursor_y, mid_y, lerp_neutral_green(pc))
        } else {
            (mid_y, cursor_y, lerp_neutral_blue(-pc))
        };
        if (bot - top).abs() > 0.5 {
            let filled = egui::Rect::from_min_max(
                egui::pos2(rect.left(), top),
                egui::pos2(rect.right(), bot),
            );
            p.rect_filled(filled, 2.0, color);
        }
        // Cursor: always visible, even at exactly the midpoint.
        p.line_segment(
            [
                egui::pos2(rect.left() - 1.0, cursor_y),
                egui::pos2(rect.right() + 1.0, cursor_y),
            ],
            egui::Stroke::new(2.0, INK),
        );
    }
}

/// Lerp neutral (track gray) → bright green by `f` in [0, 1].
fn lerp_neutral_green(f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    let r = (210.0 + (95.0 - 210.0) * f) as u8;
    let g = (210.0 + (180.0 - 210.0) * f) as u8;
    let b = (210.0 + (120.0 - 210.0) * f) as u8;
    Color32::from_rgb(r, g, b)
}

/// Lerp neutral (track gray) → starting-zone blue by `f` in [0, 1].
fn lerp_neutral_blue(f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    let r = (210.0 + (110.0 - 210.0) * f) as u8;
    let g = (210.0 + (160.0 - 210.0) * f) as u8;
    let b = (210.0 + (210.0 - 210.0) * f) as u8;
    Color32::from_rgb(r, g, b)
}
