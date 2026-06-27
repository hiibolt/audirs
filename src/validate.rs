//! Offline validation harness (Phase 2 / Phase 3 validation gates).
//!
//! Feeds a WAV file through the *exact same* DSP path the live app uses and
//! prints per-frame and summary F0 / F1 / F2 / weight, so the output can be
//! compared against a reference (Praat) on the same recording.
//!
//! Usage: `audirs --analyze path/to/sustained_vowel.wav`

use crate::dsp::{DspConfig, DspEngine};
use crate::types::VoiceFrame;

pub fn run(path: &str) -> Result<(), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let sample_rate = spec.sample_rate;

    // Decode to mono f32 regardless of source format.
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / max)
                .collect()
        }
    };
    let mono: Vec<f32> = if channels <= 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|c| c.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    let cfg = DspConfig::for_sample_rate(sample_rate);
    let mut engine = DspEngine::new(cfg)?;
    // Analyze everything; this is offline reference material, not a live mic.
    engine.set_silence_rms(0.0);

    println!("# audirs --analyze {path}");
    println!(
        "# {sample_rate} Hz, {channels} ch, {} samples ({:.2}s), frame={}, hop={}, lpc_order={}",
        mono.len(),
        mono.len() as f32 / sample_rate as f32,
        cfg.frame_size,
        cfg.hop_size,
        cfg.lpc_order,
    );
    println!("# {:>8}  {:>7}  {:>7}  {:>7}  {:>7}", "t_ms", "f0", "f1", "f2", "H1-H2");

    let mut frames: Vec<VoiceFrame> = Vec::new();
    let mut start = 0usize;
    while start + cfg.frame_size <= mono.len() {
        let window = &mono[start..start + cfg.frame_size];
        let ts_ms = (start as u64 * 1000) / sample_rate as u64;
        let f = engine.analyze(window, ts_ms);
        frames.push(f);
        start += cfg.hop_size;
    }

    // Print every Nth frame so a few seconds of audio stays readable.
    let stride = (frames.len() / 40).max(1);
    for f in frames.iter().step_by(stride) {
        println!(
            "  {:>8}  {:>7}  {:>7}  {:>7}  {:>7}",
            f.timestamp_ms,
            fmt(f.f0),
            fmt(f.f1),
            fmt(f.f2),
            fmt(f.weight),
        );
    }

    // Summary medians over voiced frames — the numbers to diff against Praat.
    println!("\n# --- summary (median over voiced frames) ---");
    report("F0   (Hz)", median(frames.iter().filter_map(|f| f.f0)));
    report("F1   (Hz)", median(frames.iter().filter_map(|f| f.f1)));
    report("F2   (Hz)", median(frames.iter().filter_map(|f| f.f2)));
    report("H1-H2 (dB)", median(frames.iter().filter_map(|f| f.weight)));
    let voiced = frames.iter().filter(|f| f.f0.is_some()).count();
    println!(
        "# voiced frames: {voiced}/{} ({:.0}%)",
        frames.len(),
        100.0 * voiced as f32 / frames.len().max(1) as f32
    );
    Ok(())
}

fn fmt(v: Option<f32>) -> String {
    match v {
        Some(x) => format!("{x:.1}"),
        None => "—".to_string(),
    }
}

fn report(label: &str, m: Option<f32>) {
    match m {
        Some(x) => println!("# {label}: {x:.1}"),
        None => println!("# {label}: (none)"),
    }
}

fn median(it: impl Iterator<Item = f32>) -> Option<f32> {
    let mut v: Vec<f32> = it.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}
