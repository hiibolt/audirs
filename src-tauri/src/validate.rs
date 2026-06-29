//! Offline validation harness (Phase 2 / Phase 3 validation gates).
//!
//! Feeds a WAV file through the *exact same* DSP path the live app uses and
//! prints per-frame and summary F0 / F1 / F2 / weight, so the output can be
//! compared against a reference (Praat) on the same recording.
//!
//! Usage: `audirs --analyze path/to/sustained_vowel.{wav,ogg,mp3}`

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::dsp::{DspConfig, DspEngine};
use crate::types::VoiceFrame;

pub fn run(path: &str) -> Result<(), String> {
    let (mono, sample_rate, channels) = decode_to_mono(path)?;

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

/// Diagnostic: sweep LPC order on a steady mid-file window and dump the raw
/// formant output (f1/f2/f3/confidence) straight from loqa, unfiltered.
pub fn sweep(path: &str) -> Result<(), String> {
    let (mono, sample_rate, _ch) = decode_to_mono(path)?;
    println!("# formant-sweep {path}  ({sample_rate} Hz, {} samples)", mono.len());
    // Take a 2048-sample window from the middle (steady portion).
    let frame = 2048.min(mono.len());
    let start = mono.len().saturating_sub(frame) / 2;
    let window = &mono[start..start + frame];
    println!("# {:>5}  {:>7}  {:>7}  {:>7}  {:>5}", "order", "f1", "f2", "f3", "conf");
    for order in [8usize, 10, 12, 14, 16, 18, 20, 24] {
        match loqa_voice_dsp::formants::extract_formants(window, sample_rate, order) {
            Ok(r) => println!(
                "  {order:>5}  {:>7.1}  {:>7.1}  {:>7.1}  {:>5.2}",
                r.f1, r.f2, r.f3, r.confidence
            ),
            Err(e) => println!("  {order:>5}  error: {e}"),
        }
    }
    Ok(())
}

/// Decode any supported file (wav/ogg/mp3) to mono f32 + its sample rate.
/// Returns `(samples, sample_rate, source_channel_count)`.
fn decode_to_mono(path: &str) -> Result<(Vec<f32>, u32, usize), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe failed (unsupported format?): {e}"))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| "no default audio track".to_string())?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("no decoder for codec: {e}"))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = 1usize;
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) => break, // end of stream
            Err(e) => return Err(format!("read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                if sbuf.is_none() {
                    let spec = *audio_buf.spec();
                    sample_rate = spec.rate;
                    channels = spec.channels.count().max(1);
                    sbuf = Some(SampleBuffer::<f32>::new(audio_buf.capacity() as u64, spec));
                }
                if let Some(buf) = sbuf.as_mut() {
                    buf.copy_interleaved_ref(audio_buf);
                    interleaved.extend_from_slice(buf.samples());
                }
            }
            Err(SymError::DecodeError(_)) => continue, // skip bad packet
            Err(e) => return Err(format!("decode error: {e}")),
        }
    }

    if sample_rate == 0 || interleaved.is_empty() {
        return Err("decoded zero audio samples".to_string());
    }

    let mono: Vec<f32> = if channels <= 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|c| c.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    Ok((mono, sample_rate, channels))
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
