//! Realtime audio capture and the analysis worker.
//!
//! Architecture:
//!   cpal RT callback --(rtrb f32)--> worker thread --(rtrb VoiceFrame)--> drain()
//!
//! The RT callback only downmixes to mono and pushes into a lock-free ring
//! buffer — no allocation, no locks, no blocking. All DSP runs on the worker.
//!
//! Tauri note: cpal's `Stream` is `!Send`, but `tauri::State` must be
//! `Send + Sync`. So the stream lives parked on its own *host thread* and never
//! crosses a thread boundary; `AudioHandle` holds only `Send` pieces (the frame
//! consumer + control atomics), making it safe to store in shared state.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, RingBuffer};

use crate::dsp::{DspConfig, DspEngine};
use crate::types::VoiceFrame;

/// Live-adjustable capture controls shared (lock-free) between UI and worker.
/// Values are stored as `f32` bit patterns in atomics.
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

/// `Send`-safe handle to a running capture session. The cpal `Stream` is parked
/// on the host thread; this holds only `Send` pieces. Dropping it stops capture.
pub struct AudioHandle {
    pub sample_rate: u32,
    pub device_name: String,
    controls: Arc<AudioControls>,
    lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    frames: Consumer<VoiceFrame>,
    host: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<()>>,
}

impl AudioHandle {
    /// True once the input stream reported an error (e.g. device unplugged).
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
    pub fn controls(&self) -> &Arc<AudioControls> {
        &self.controls
    }
    /// Drain every frame produced since the previous call.
    pub fn drain(&mut self) -> Vec<VoiceFrame> {
        let mut out = Vec::new();
        while let Ok(f) = self.frames.pop() {
            out.push(f);
        }
        out
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.host.take() {
            let _ = h.join();
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// Start capture + analysis. Device selection and stream construction happen on
/// the host thread (so the `!Send` `Device`/`Stream` never move across threads);
/// the chosen name + sample rate are reported back over a one-shot channel.
pub fn start(preferred: Option<String>) -> Result<AudioHandle, String> {
    // Generous fixed capacity (~1s even at 192 kHz) — sized before we know the
    // real rate, which the host thread discovers.
    let (mut sample_prod, sample_cons) = RingBuffer::<f32>::new(192_000);
    let (frame_prod, frame_cons) = RingBuffer::<VoiceFrame>::new(4096);

    let lost = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let controls = Arc::new(AudioControls::new());

    let (ready_tx, ready_rx) = mpsc::channel::<Result<(String, u32), String>>();
    let lost_cb = lost.clone();
    let stop_host = stop.clone();

    let host_thread = thread::spawn(move || {
        let init = (|| -> Result<(String, u32, usize, cpal::StreamConfig), String> {
            let host = cpal::default_host();
            let device = match preferred.as_deref() {
                Some(name) => host
                    .input_devices()
                    .ok()
                    .and_then(|mut d| {
                        d.find(|x| {
                            x.description().ok().map(|y| y.name() == name).unwrap_or(false)
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
            if supported.sample_format() != SampleFormat::F32 {
                return Err(format!(
                    "input sample format is {:?}, expected F32",
                    supported.sample_format()
                ));
            }
            let sample_rate = supported.sample_rate();
            let channels = supported.channels() as usize;
            let config: cpal::StreamConfig = supported.into();

            // Build the stream here so it never leaves this thread.
            let data_cb = move |data: &[f32], _: &cpal::InputCallbackInfo| {
                for frame in data.chunks(channels) {
                    let mut sum = 0.0f32;
                    for &s in frame {
                        sum += s;
                    }
                    let _ = sample_prod.push(sum / channels as f32);
                }
            };
            let err_cb = move |e| {
                eprintln!("audio stream error: {e}");
                lost_cb.store(true, Ordering::Relaxed);
            };
            let stream = device
                .build_input_stream::<f32, _, _>(config.clone(), data_cb, err_cb, None)
                .map_err(|e| format!("failed to build input stream: {e}"))?;
            stream.play().map_err(|e| format!("failed to start stream: {e}"))?;

            // Park the stream alive until asked to stop.
            let _keep_alive = stream;
            let _ = ready_tx.send(Ok((device_name.clone(), sample_rate)));
            while !stop_host.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            Ok((device_name, sample_rate, channels, config))
        })();

        if let Err(e) = init {
            let _ = ready_tx.send(Err(e));
        }
    });

    let (device_name, sample_rate) = match ready_rx.recv() {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("audio host thread exited".to_string()),
    };

    let cfg = DspConfig::for_sample_rate(sample_rate);
    let worker = spawn_worker(cfg, sample_cons, frame_prod, stop.clone(), controls.clone())?;

    Ok(AudioHandle {
        sample_rate,
        device_name,
        controls,
        lost,
        stop,
        frames: frame_cons,
        host: Some(host_thread),
        worker: Some(worker),
    })
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
        let mut acc: Vec<f32> = Vec::with_capacity(frame_size * 4);
        let mut window: Vec<f32> = vec![0.0; frame_size];
        let mut consumed: u64 = 0;

        while !stop.load(Ordering::Relaxed) {
            let mut got_any = false;
            while let Ok(s) = samples_in.pop() {
                acc.push(s);
                got_any = true;
            }

            while acc.len() >= frame_size {
                window.copy_from_slice(&acc[..frame_size]);
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
                thread::sleep(Duration::from_millis(5));
            }
        }
    });

    Ok(handle)
}
