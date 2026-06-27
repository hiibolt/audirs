//! Custom LPC formant extractor.
//!
//! Built in-house after loqa-voice-dsp 0.5.0's formant path failed ground-truth
//! validation (errored on clean vowels; order-sensitive wrong values otherwise).
//! Validate any change here against the synthetic harness — known F1/F2/F3 in,
//! recovered F1/F2/F3 out.
//!
//! Pipeline: resample→pre-emphasis→Hamming→autocorrelation→Levinson-Durbin→
//! root-find A(z)→formants from the roots' angle/radius.

const ANALYSIS_RATE: f64 = 16_000.0;
const LPC_ORDER: usize = 18;
/// Max formant bandwidth to accept (Hz). Back/rounded vowels (/u/, /o/) have
/// broader low formants, so too tight a cap rejects the real F2 and lets a
/// spurious high-frequency pole take its place.
const MAX_BANDWIDTH: f64 = 1000.0;

/// Formant frequencies in Hz, ascending. Empty if the frame yields no
/// resonances that pass the bandwidth/frequency sanity checks.
pub fn extract(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let mut sig = resample(samples, sample_rate as f64, ANALYSIS_RATE);
    if sig.len() <= LPC_ORDER + 1 {
        return Vec::new();
    }
    pre_emphasis(&mut sig, 0.97);
    hamming(&mut sig);

    let r = autocorr(&sig, LPC_ORDER);
    let Some(a) = levinson(&r, LPC_ORDER) else {
        return Vec::new();
    };

    // A(z) = 1 + a[1] z^-1 + ... + a[p] z^-p. As a polynomial in z (leading
    // first): z^p + a[1] z^(p-1) + ... + a[p]. Coeffs are exactly `a`.
    let roots = durand_kerner(&a);

    let mut formants: Vec<f32> = Vec::new();
    for z in roots {
        if z.im <= 0.0 {
            continue; // one of each conjugate pair
        }
        let mag = z.abs();
        if mag >= 1.0 || mag < 0.7 {
            continue; // unstable or too-damped to be a formant
        }
        let freq = z.arg() * ANALYSIS_RATE / std::f64::consts::TAU;
        let bw = -(ANALYSIS_RATE / std::f64::consts::PI) * mag.ln();
        if freq > 90.0 && freq < 5_000.0 && bw < MAX_BANDWIDTH {
            formants.push(freq as f32);
        }
    }
    formants.sort_by(|a, b| a.partial_cmp(b).unwrap());
    formants
}

/// Linear-interpolation resample. Speech formant energy is below ~5 kHz, so the
/// lack of a steep anti-alias filter is acceptable for this band.
fn resample(x: &[f32], from: f64, to: f64) -> Vec<f64> {
    if (from - to).abs() < 1.0 {
        return x.iter().map(|&v| v as f64).collect();
    }
    let ratio = to / from;
    let n_out = (x.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(n_out);
    for i in 0..n_out {
        let src = i as f64 / ratio;
        let i0 = src.floor() as usize;
        let frac = src - i0 as f64;
        let s0 = x.get(i0).copied().unwrap_or(0.0) as f64;
        let s1 = x.get(i0 + 1).copied().map(|v| v as f64).unwrap_or(s0);
        out.push(s0 * (1.0 - frac) + s1 * frac);
    }
    out
}

fn pre_emphasis(x: &mut [f64], coeff: f64) {
    for i in (1..x.len()).rev() {
        x[i] -= coeff * x[i - 1];
    }
}

fn hamming(x: &mut [f64]) {
    let n = x.len();
    if n < 2 {
        return;
    }
    for (i, s) in x.iter_mut().enumerate() {
        let w = 0.54 - 0.46 * (std::f64::consts::TAU * i as f64 / (n as f64 - 1.0)).cos();
        *s *= w;
    }
}

fn autocorr(x: &[f64], p: usize) -> Vec<f64> {
    let mut r = vec![0.0; p + 1];
    for (lag, rl) in r.iter_mut().enumerate() {
        let mut s = 0.0;
        for i in lag..x.len() {
            s += x[i] * x[i - lag];
        }
        *rl = s;
    }
    r
}

/// Levinson-Durbin recursion. Returns the error-filter coefficients
/// `a` with `a[0] == 1.0` and `a[1..=p]` such that
/// `A(z) = 1 + a[1] z^-1 + ... + a[p] z^-p`. `None` if the recursion is unstable.
fn levinson(r: &[f64], p: usize) -> Option<Vec<f64>> {
    if r[0].abs() < 1e-12 {
        return None;
    }
    let mut a = vec![0.0; p + 1];
    a[0] = 1.0;
    let mut e = r[0];
    for i in 1..=p {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / e;
        // Symmetric update using the previous iteration's coefficients.
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        e *= 1.0 - k * k;
        if e <= 0.0 {
            return None;
        }
    }
    Some(a)
}

// --- minimal complex arithmetic + Durand-Kerner root finder ---

#[derive(Clone, Copy)]
struct Cx {
    re: f64,
    im: f64,
}

impl Cx {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn add(self, o: Cx) -> Cx {
        Cx::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: Cx) -> Cx {
        Cx::new(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: Cx) -> Cx {
        Cx::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    fn div(self, o: Cx) -> Cx {
        let d = o.re * o.re + o.im * o.im;
        Cx::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
}

/// Find all roots of the polynomial whose coefficients are `coeffs`
/// (leading-coefficient first): `coeffs[0] z^n + ... + coeffs[n]`.
fn durand_kerner(coeffs: &[f64]) -> Vec<Cx> {
    let n = coeffs.len() - 1; // degree
    if n == 0 {
        return Vec::new();
    }
    let c: Vec<Cx> = coeffs.iter().map(|&v| Cx::new(v, 0.0)).collect();

    // Spread initial guesses around a circle (a fixed, non-real seed avoids
    // pathological symmetric configurations).
    let seed = Cx::new(0.4, 0.9);
    let mut roots: Vec<Cx> = Vec::with_capacity(n);
    let mut acc = Cx::new(1.0, 0.0);
    for _ in 0..n {
        roots.push(acc);
        acc = acc.mul(seed);
    }

    for _ in 0..200 {
        let mut max_delta = 0.0f64;
        for i in 0..n {
            let p = horner(&c, roots[i]);
            // denominator = leading * prod_{j!=i} (r_i - r_j)
            let mut denom = c[0];
            for j in 0..n {
                if j != i {
                    denom = denom.mul(roots[i].sub(roots[j]));
                }
            }
            let delta = p.div(denom);
            roots[i] = roots[i].sub(delta);
            max_delta = max_delta.max(delta.abs());
        }
        if max_delta < 1e-10 {
            break;
        }
    }
    roots
}

fn horner(coeffs: &[Cx], z: Cx) -> Cx {
    let mut acc = coeffs[0];
    for c in &coeffs[1..] {
        acc = acc.mul(z).add(*c);
    }
    acc
}
