//! Tauri backend for audirs. Owns realtime capture + DSP and exposes a small
//! command surface; the Svelte frontend drives all visualization, smoothing,
//! and session aggregation by draining frames each animation tick.

mod audio;
mod dsp;
mod settings;
mod targets;
mod types;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Manager, State};

use audio::AudioHandle;
use settings::Settings;
use targets::Targets;
use types::VoiceFrame;

struct AppState {
    audio: Mutex<Option<AudioHandle>>,
    settings: Mutex<Settings>,
}

/// Capture status for the UI banner / device label.
#[derive(Serialize)]
struct Status {
    listening: bool,
    device_name: String,
    sample_rate: u32,
    lost: bool,
}

/// The active goal band plus the starting (opposite-gender) band for overlays.
#[derive(Serialize)]
struct Zones {
    effective: Targets,
    starting: Targets,
}

#[tauri::command]
fn list_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn zones(state: State<AppState>) -> Zones {
    let s = state.settings.lock().unwrap();
    Zones {
        effective: s.effective_targets(),
        starting: s.starting_targets(),
    }
}

#[tauri::command]
fn save_settings(state: State<AppState>, new: Settings) {
    if let Some(h) = state.audio.lock().unwrap().as_ref() {
        h.controls().set_gain(new.gain);
        h.controls().set_silence_rms(new.threshold);
    }
    new.save();
    *state.settings.lock().unwrap() = new;
}

#[tauri::command]
fn start_capture(state: State<AppState>, device: Option<String>) -> Result<Status, String> {
    // Drop any existing session first (its Drop stops the stream + worker).
    *state.audio.lock().unwrap() = None;

    let handle = audio::start(device)?;
    let s = state.settings.lock().unwrap().clone();
    handle.controls().set_gain(s.gain);
    handle.controls().set_silence_rms(s.threshold);

    let status = Status {
        listening: true,
        device_name: handle.device_name.clone(),
        sample_rate: handle.sample_rate,
        lost: false,
    };
    *state.audio.lock().unwrap() = Some(handle);
    Ok(status)
}

#[tauri::command]
fn stop_capture(state: State<AppState>) {
    *state.audio.lock().unwrap() = None;
}

#[tauri::command]
fn status(state: State<AppState>) -> Status {
    match state.audio.lock().unwrap().as_ref() {
        Some(h) => Status {
            listening: true,
            device_name: h.device_name.clone(),
            sample_rate: h.sample_rate,
            lost: h.lost(),
        },
        None => Status {
            listening: false,
            device_name: String::new(),
            sample_rate: 0,
            lost: false,
        },
    }
}

/// Drain all frames produced since the previous call (frontend polls per rAF).
#[tauri::command]
fn drain(state: State<AppState>) -> Vec<VoiceFrame> {
    match state.audio.lock().unwrap().as_mut() {
        Some(h) => h.drain(),
        None => Vec::new(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let settings = Settings::load();
            app.manage(AppState {
                audio: Mutex::new(None),
                settings: Mutex::new(settings),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            get_settings,
            zones,
            save_settings,
            start_capture,
            stop_capture,
            status,
            drain,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
