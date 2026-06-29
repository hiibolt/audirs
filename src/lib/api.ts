// Typed wrappers over the Tauri command surface (see src-tauri/src/lib.rs).
import { invoke } from "@tauri-apps/api/core";

export interface VoiceFrame {
  timestamp_ms: number;
  f0: number | null;
  f1: number | null;
  f2: number | null;
  weight: number | null;
  rms: number;
}

export interface Targets {
  pitch_lo: number;
  pitch_hi: number;
  f1_lo: number;
  f1_hi: number;
  f2_lo: number;
  f2_hi: number;
  weight_lo: number;
  weight_hi: number;
}

export interface Zones {
  effective: Targets;
  starting: Targets;
}

export type Gender = "Male" | "Female";

export interface Settings {
  gain: number;
  threshold: number;
  device: string | null;
  target_gender: Gender;
  goal_percent: number;
  show_starting: boolean;
}

export interface Status {
  listening: boolean;
  device_name: string;
  sample_rate: number;
  lost: boolean;
}

export const api = {
  listDevices: () => invoke<string[]>("list_devices"),
  getSettings: () => invoke<Settings>("get_settings"),
  zones: () => invoke<Zones>("zones"),
  saveSettings: (s: Settings) => invoke<void>("save_settings", { new: s }),
  startCapture: (device: string | null) =>
    invoke<Status>("start_capture", { device }),
  stopCapture: () => invoke<void>("stop_capture"),
  status: () => invoke<Status>("status"),
  drain: () => invoke<VoiceFrame[]>("drain"),
};
