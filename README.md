# audirs
Rudimentary voice training software to help transfems/transmascs visualize and train format, pitch, and weight.

<img width="939" height="870" alt="image" src="https://github.com/user-attachments/assets/7d7691da-22c4-405b-ac7e-06f6744193f4" />

Features include:
- **Safety**:
  - Lets you choose the percent to the target zone you want to be in. Very important since it's unsafe to train for 100% accuracy upfront - you *will* hurt yourself if you try!
- **Comparison**:
  - You can enable/disable the target zone to compare where your stats line up against the target
  - You can view pitch, formant, and weight comparitive to your goal in real-time and over the last 5/30s. 
- **Graphs**:
  - Rolling pitch graph - shows both target/starting zone
  - Static formant graph - shows both target/starting zone, plots F1 against F2
  - Static weight graph - shows both target/starting zone
  - Overall pitch graph - shows both target/starting zone over the course of the entire session
- **Microphone**:
  - Input device select
  - Volume booster
  - Silence threshold

## Getting Started
### Windows
**Prerequisites**:
- [Rust](https://www.rust-lang.org/en-US) 

Download the source for this repository either by using the green Code button above, or the [Git CLI](https://kagi.com/search?q=git+for+windows).

Lastly, run `cargo run` from inside the source directory.

*If you get an error about the build script not being able to run, you may have [SAC](https://support.microsoft.com/en-US/Windows/Security/Threat-Malware-Protection/smart-app-control-frequently-asked-questions) enabled. You can either disable it in Windows settings, or run this software from a directory that's whitelisted on your machine.*
### Linux
**Prerequisites**:
- [Rust](https://www.rust-lang.org/en-US) 

Run the following:
```bash
git clone https://github.com/hiibolt/audirs.git
cd audirs
cargo run
```

*If you're a Nix user with Flakes enabled, you can run `nix develop` and `cargo run` without needing to install the Rust toolchain globally. This Nix flake also captures all dependencies you may need for systems that might not otherwise have them, like NixOS!*
