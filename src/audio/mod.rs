//! Realtime audio capture and the analysis worker.
//!
//! Architecture (build-plan principles #1 and #2):
//!   cpal RT callback  --(rtrb f32 samples)-->  worker thread  --(rtrb VoiceFrame)-->  UI
//!
//! The RT callback does the absolute minimum: downmix to mono and push into a
//! lock-free ring buffer. No allocation, no locks, no blocking, no panicking.
//! All DSP happens on the non-realtime worker thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use rtrb::{Consumer, RingBuffer};

use crate::dsp::{DspConfig, DspEngine};
use crate::types::VoiceFrame;

/// Live-adjustable capture controls shared (lock-free) between the UI and the
/// analysis worker. Values are stored as `f32` bit patterns in atomics.
pub struct AudioControls {
    gain: AtomicU32,
    silence_rms: AtomicU32,
}

impl AudioControls {
    fn new() -> Self {
        Self {
            gain: AtomicU32::new(1.0f32.to_bits()),
            silence_rms: AtomicU32::new(0.01f32.to_bits()),
        }
    }
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
    pub fn set_gain(&self, v: f32) {
        self.gain.store(v.to_bits(), Ordering::Relaxed);
    }
    pub fn silence_rms(&self) -> f32 {
        f32::from_bits(self.silence_rms.load(Ordering::Relaxed))
    }
    pub fn set_silence_rms(&self, v: f32) {
        self.silence_rms.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Owns the live capture stream and worker thread. Dropping it stops capture.
pub struct AudioEngine {
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    lost: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub device_name: String,
    pub controls: Arc<AudioControls>,
}

impl AudioEngine {
    /// True once the input stream has reported an error (e.g. device unplugged).
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

/// Names of available input devices (for the device picker).
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devs) => devs
            .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// Open the default input device and start capture + analysis.
///
/// Returns the engine (keep it alive!) and a consumer the UI drains for frames.
pub fn start(
    preferred: Option<&str>,
) -> Result<(AudioEngine, Consumer<VoiceFrame>), String> {
    let host = cpal::default_host();
    // Use the preferred device by name if present, else the system default.
    let device = match preferred {
        Some(name) => host
            .input_devices()
            .ok()
            .and_then(|mut devs| {
                devs.find(|d| {
                    d.description().ok().map(|x| x.name() == name).unwrap_or(false)
                })
            })
            .or_else(|| host.default_input_device())
            .ok_or_else(|| "no input device available".to_string())?,
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input device found".to_string())?,
    };
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "input".to_string());

    let supported = device
        .default_input_config()
        .map_err(|e| format!("no default input config: {e}"))?;

    // Per principle "report, don't silently substitute": we only handle f32
    // here. Other formats are flagged rather than quietly converted.
    if supported.sample_format() != SampleFormat::F32 {
        return Err(format!(
            "input device sample format is {:?}, expected F32 (extend in Phase 6)",
            supported.sample_format()
        ));
    }

    let sample_rate = supported.sample_rate(); // cpal 0.18: SampleRate = u32
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.into();

    // --- ring buffers ---
    // ~1s of audio headroom between RT callback and worker.
    let (mut sample_prod, sample_cons) = RingBuffer::<f32>::new(sample_rate as usize);
    // Frame queue to the UI: generous so the UI can fall behind briefly.
    let (frame_prod, frame_cons) = RingBuffer::<VoiceFrame>::new(2048);

    // --- RT callback: downmix to mono, push, nothing else ---
    let data_cb = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        for frame in data.chunks(channels) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s;
            }
            let mono = sum / channels as f32;
            // Non-blocking. On overflow we drop — never block the RT thread.
            let _ = sample_prod.push(mono);
        }
    };
    let lost = Arc::new(AtomicBool::new(false));
    let lost_cb = lost.clone();
    let err_cb = move |e| {
        eprintln!("audio stream error: {e}");
        lost_cb.store(true, Ordering::Relaxed);
    };

    let stream = device
        .build_input_stream::<f32, _, _>(config, data_cb, err_cb, None)
        .map_err(|e| format!("failed to build input stream: {e}"))?;
    stream.play().map_err(|e| format!("failed to start stream: {e}"))?;

    // --- worker thread ---
    let cfg = DspConfig::for_sample_rate(sample_rate);
    let stop = Arc::new(AtomicBool::new(false));
    let controls = Arc::new(AudioControls::new());
    let worker = spawn_worker(cfg, sample_cons, frame_prod, stop.clone(), controls.clone())?;

    Ok((
        AudioEngine {
            _stream: stream,
            stop,
            worker: Some(worker),
            lost,
            sample_rate,
            device_name,
            controls,
        },
        frame_cons,
    ))
}

fn spawn_worker(
    cfg: DspConfig,
    mut samples_in: Consumer<f32>,
    mut frames_out: rtrb::Producer<VoiceFrame>,
    stop: Arc<AtomicBool>,
    controls: Arc<AudioControls>,
) -> Result<JoinHandle<()>, String> {
    let mut engine = DspEngine::new(cfg)?;
    let frame_size = cfg.frame_size;
    let hop_size = cfg.hop_size;

    let handle = thread::spawn(move || {
        // Rolling accumulator of mono samples awaiting analysis.
        let mut acc: Vec<f32> = Vec::with_capacity(frame_size * 4);
        // Reusable analysis window — avoids per-iteration allocation.
        let mut window: Vec<f32> = vec![0.0; frame_size];
        // Total samples removed from the front of `acc` — drives timestamps.
        let mut consumed: u64 = 0;

        while !stop.load(Ordering::Relaxed) {
            // Drain everything currently available from the RT side.
            let mut got_any = false;
            while let Ok(s) = samples_in.pop() {
                acc.push(s);
                got_any = true;
            }

            // Emit a frame for every full hop we can cover.
            while acc.len() >= frame_size {
                window.copy_from_slice(&acc[..frame_size]);
                // Apply live mic boost before analysis.
                let gain = controls.gain();
                if gain != 1.0 {
                    for s in window.iter_mut() {
                        *s *= gain;
                    }
                }
                engine.set_silence_rms(controls.silence_rms());
                let ts_ms = (consumed * 1000) / cfg.sample_rate as u64;
                let frame = engine.analyze(&window, ts_ms);
                let _ = frames_out.push(frame);

                acc.drain(..hop_size);
                consumed += hop_size as u64;
            }

            if !got_any {
                // Nothing to do — yield briefly. Worker is non-realtime.
                thread::sleep(Duration::from_millis(5));
            }
        }
    });

    Ok(handle)
}
