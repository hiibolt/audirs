//! Thin wrapper over `loqa-voice-dsp` that turns a window of mono samples
//! into a `VoiceFrame`. This is the ONLY place DSP-library types appear.
//!
//! Accuracy note (build-plan principle #4): the numbers this module emits
//! drive user-facing feedback about a deeply personal metric. The values
//! MUST be validated against Praat (see Phase 2 / Phase 3 validation gates)
//! before they are trusted. Until that's done, treat readouts as provisional.

use loqa_voice_dsp::analyzer::VoiceAnalyzer;
use loqa_voice_dsp::config::AnalysisConfig;
use loqa_voice_dsp::h1h2::calculate_h1h2;

use crate::types::VoiceFrame;

pub mod formants;

/// Tunables for analysis. Sensible defaults are derived from the sample rate.
#[derive(Clone, Copy, Debug)]
pub struct DspConfig {
    pub sample_rate: u32,
    pub frame_size: usize,
    pub hop_size: usize,
    pub min_frequency: f32,
    pub max_frequency: f32,
    /// LPC order for formant estimation. Rule of thumb: 2 + sample_rate/1000.
    pub lpc_order: usize,
    /// Linear RMS below which a frame is treated as silence (no analysis).
    pub silence_rms: f32,
    /// Minimum pitch confidence to accept an f0 as voiced.
    pub min_confidence: f32,
}

impl DspConfig {
    pub fn for_sample_rate(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            frame_size: 2048,
            hop_size: 512,
            min_frequency: 65.0,
            max_frequency: 500.0,
            // loqa downsamples >20 kHz to 16 kHz internally before LPC, so the
            // order must suit a 16 kHz signal (~2 + 16). A device-rate order
            // (e.g. 50 at 48 kHz) makes the LPC unstable and yields garbage
            // formants that get filtered out — leaving the scatter empty.
            lpc_order: 16,
            silence_rms: 0.01,
            min_confidence: 0.5,
        }
    }
}

/// Stateful analyzer reused across frames (the pitch detector is stateful).
pub struct DspEngine {
    cfg: DspConfig,
    analyzer: VoiceAnalyzer,
}

impl DspEngine {
    pub fn new(cfg: DspConfig) -> Result<Self, String> {
        // AnalysisConfig fields are public — set them directly so pitch search
        // respects our frequency range and confidence floor.
        let mut analysis = AnalysisConfig::default();
        analysis.sample_rate = cfg.sample_rate;
        analysis.frame_size = cfg.frame_size;
        analysis.hop_size = cfg.hop_size;
        analysis.min_frequency = cfg.min_frequency;
        analysis.max_frequency = cfg.max_frequency;
        analysis.min_confidence = cfg.min_confidence;
        let analyzer = VoiceAnalyzer::new(analysis)?;
        Ok(Self { cfg, analyzer })
    }

    /// Live-adjustable silence gate (driven from the UI threshold slider).
    pub fn set_silence_rms(&mut self, v: f32) {
        self.cfg.silence_rms = v;
    }

    /// Analyze one window of mono samples into a `VoiceFrame`.
    ///
    /// Silent frames short-circuit to a level-only frame. Unvoiced frames
    /// (pitch detector says so) carry `None` for every perceptual metric,
    /// so the UI renders a gap rather than garbage.
    pub fn analyze(&mut self, samples: &[f32], timestamp_ms: u64) -> VoiceFrame {
        let rms = rms(samples);

        // Silence gate — don't let background noise produce garbage readouts.
        if rms < self.cfg.silence_rms {
            return VoiceFrame::silent(timestamp_ms, rms);
        }

        // --- Pitch ---
        let pitch = self.analyzer.process_frame(samples).ok();
        let f0 = match pitch {
            Some(p) if p.is_voiced && p.confidence >= self.cfg.min_confidence => {
                Some(p.frequency)
            }
            _ => None,
        };

        // No voiced pitch => treat formants/weight as meaningless this frame.
        if f0.is_none() {
            return VoiceFrame::silent(timestamp_ms, rms);
        }

        // --- Formants (our own LPC extractor) ---
        let fmts = formants::extract(samples, self.cfg.sample_rate);
        let f1 = fmts.first().copied().and_then(sane_formant);
        let f2 = fmts.get(1).copied().and_then(sane_formant);

        // --- Vocal weight (H1-H2) ---
        let weight = calculate_h1h2(samples, self.cfg.sample_rate, f0)
            .ok()
            .map(|r| r.h1h2);

        VoiceFrame {
            timestamp_ms,
            f0,
            f1,
            f2,
            weight,
            rms,
        }
    }
}

/// Linear RMS of a sample window.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Reject obviously-bogus formant readings (0 Hz, NaN) so the UI never
/// plots a point that lies about where the voice sits.
fn sane_formant(hz: f32) -> Option<f32> {
    if hz.is_finite() && hz > 90.0 && hz < 5000.0 {
        Some(hz)
    } else {
        None
    }
}
