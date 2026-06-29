//! Target reference bands and gender presets. Extracted from the egui UI so the
//! backend (and settings) can own them without any UI dependency.

use serde::{Deserialize, Serialize};

/// Target reference ranges for each metric. Population *starting points*, not
/// goals — the frontend must present them as such.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Targets {
    pub pitch_lo: f32,
    pub pitch_hi: f32,
    pub f1_lo: f32,
    pub f1_hi: f32,
    pub f2_lo: f32,
    pub f2_hi: f32,
    pub weight_lo: f32,
    pub weight_hi: f32,
}

impl Default for Targets {
    fn default() -> Self {
        Self::feminine()
    }
}

impl Targets {
    /// Comfortable feminine reference band.
    pub const fn feminine() -> Self {
        Self {
            pitch_lo: 165.0,
            pitch_hi: 220.0,
            f1_lo: 350.0,
            f1_hi: 850.0,
            f2_lo: 1700.0,
            f2_hi: 2600.0,
            weight_lo: 3.0,
            weight_hi: 14.0,
        }
    }

    /// Comfortable masculine reference band.
    pub const fn masculine() -> Self {
        Self {
            pitch_lo: 85.0,
            pitch_hi: 155.0,
            f1_lo: 300.0,
            f1_hi: 750.0,
            f2_lo: 1100.0,
            f2_hi: 1900.0,
            weight_lo: -2.0,
            weight_hi: 6.0,
        }
    }

    /// Linear blend between `from` and `to` band edges, by `t` in [0, 1].
    pub fn lerp(from: Self, to: Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            pitch_lo: l(from.pitch_lo, to.pitch_lo),
            pitch_hi: l(from.pitch_hi, to.pitch_hi),
            f1_lo: l(from.f1_lo, to.f1_lo),
            f1_hi: l(from.f1_hi, to.f1_hi),
            f2_lo: l(from.f2_lo, to.f2_lo),
            f2_hi: l(from.f2_hi, to.f2_hi),
            weight_lo: l(from.weight_lo, to.weight_lo),
            weight_hi: l(from.weight_hi, to.weight_hi),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
}

impl Gender {
    pub fn opposite(self) -> Self {
        match self {
            Self::Male => Self::Female,
            Self::Female => Self::Male,
        }
    }
    pub fn targets(self) -> Targets {
        match self {
            Self::Male => Targets::masculine(),
            Self::Female => Targets::feminine(),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Male => "Male",
            Self::Female => "Female",
        }
    }
}
