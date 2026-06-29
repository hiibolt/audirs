//! Shared types that cross the audio->UI boundary.
//!
//! `VoiceFrame` is the single data contract. It MUST NOT leak DSP-internal
//! types — the UI only ever sees plain numbers it can render.

/// One analyzed slice of audio, produced by the DSP worker thread and
/// consumed by the UI thread.
///
/// Every perceptual metric is `Option` because unvoiced frames (silence,
/// fricatives) have no meaningful pitch or formants. The UI renders `None`
/// as a gap, never as a zero.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct VoiceFrame {
    /// Monotonic milliseconds since capture start.
    pub timestamp_ms: u64,
    /// Fundamental frequency in Hz. `None` = unvoiced/silent.
    pub f0: Option<f32>,
    /// First formant in Hz.
    pub f1: Option<f32>,
    /// Second formant in Hz.
    pub f2: Option<f32>,
    /// H1-H2 in dB — proxy for vocal weight.
    pub weight: Option<f32>,
    /// Linear RMS amplitude, for level metering and silence gating.
    pub rms: f32,
}

impl VoiceFrame {
    /// A frame carrying only a level reading — all perceptual metrics absent.
    /// Used for silent/unvoiced frames so the UI shows gaps, not noise.
    pub fn silent(timestamp_ms: u64, rms: f32) -> Self {
        Self {
            timestamp_ms,
            f0: None,
            f1: None,
            f2: None,
            weight: None,
            rms,
        }
    }
}
