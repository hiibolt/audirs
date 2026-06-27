//! Voice Trainer — realtime feedback on pitch, formants, and vocal weight.
//! Fully local; no network, no recording, no accounts.

mod audio;
mod dsp;
mod types;
mod ui;
mod validate;

use ui::VoiceApp;

fn main() -> eframe::Result<()> {
    // Offline validation mode: `audirs --analyze file.wav` (Phase 2/3 gates).
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--analyze") {
        match args.get(pos + 1) {
            Some(path) => {
                if let Err(e) = validate::run(path) {
                    eprintln!("analyze failed: {e}");
                    std::process::exit(1);
                }
            }
            None => {
                eprintln!("usage: audirs --analyze <file.wav>");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_title("Voice Trainer"),
        ..Default::default()
    };
    eframe::run_native(
        "Voice Trainer",
        options,
        Box::new(|cc| Ok(Box::new(VoiceApp::new(cc)))),
    )
}
