// Central app store (Svelte 5 runes). Polls the backend each animation frame,
// smooths the jittery mic values (EMA), keeps a rolling 30s buffer, and
// aggregates the user-controlled session. Mirrors the egui PoC's pump/accumulate.
import { api, type Settings, type Status, type VoiceFrame, type Zones } from "./api";

const HISTORY_MS = 30_000;
const RIBBON_MS = 10_000;
const CLOUD_MS = 2_500;
const BUCKET_MS = 67; // session-trend density (~15 points/sec)
const SMOOTH = 0.18;

type Band = { lo: number; hi: number };
type Cell = { now: number | null; w5: number | null; w30: number | null; session: number | null };

interface Counts { inb: number; total: number }
// One session-trend bucket: median of each metric over a BUCKET_MS window.
export interface Bucket {
  t: number; // seconds since session start
  f0: number | null;
  f1: number | null;
  f2: number | null;
  weight: number | null;
}
interface Summary {
  durationS: number;
  medianPitch: number | null;
  pitch: number | null;
  fmt: number | null;
  weight: number | null;
}

function ema(prev: number | null, v: number | null): number | null {
  if (v == null) return null; // render a gap, but caller keeps `prev` state
  return prev == null ? v : prev + SMOOTH * (v - prev);
}

class AppStore {
  status = $state<Status>({ listening: false, device_name: "", sample_rate: 0, lost: false });
  devices = $state<string[]>([]);
  settings = $state<Settings | null>(null);
  zones = $state<Zones | null>(null);

  // Reactive snapshots recomputed each tick.
  tick = $state(0);
  current = $state<VoiceFrame | null>(null);
  pitchCell = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  fmtCell = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  weightCell = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  // Bipolar progress cells (-1 = starting-zone center, +1 = goal-zone center).
  pitchProg = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  fmtProg = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  weightProg = $state<Cell>({ now: null, w5: null, w30: null, session: null });
  sessionActive = $state(false);
  summary = $state<Summary | null>(null);

  // Plain (non-reactive) buffers read imperatively by chart components.
  frames: VoiceFrame[] = [];
  trend: Bucket[] = [];

  // EMA state (kept across short gaps so brief consonants don't reset).
  private sm = { f0: null as number | null, f1: null as number | null, f2: null as number | null, weight: null as number | null, rms: 0 };
  // Session running tallies.
  private sPitch: Counts = { inb: 0, total: 0 };
  private sFmt: Counts = { inb: 0, total: 0 };
  private sWeight: Counts = { inb: 0, total: 0 };
  // Session progress accumulators (mean of bipolar progress).
  private pPitch = { sum: 0, n: 0 };
  private pFmt = { sum: 0, n: 0 };
  private pWeight = { sum: 0, n: 0 };
  private sStart = 0;
  private sLast = 0;
  private sNextBucket = BUCKET_MS;
  private running = false;

  async init() {
    this.settings = await api.getSettings();
    this.zones = await api.zones();
    this.devices = await api.listDevices();
    this.status = await api.status();
  }

  startLoop() {
    if (this.running) return;
    this.running = true;
    const loop = async () => {
      if (!this.running) return;
      try {
        const batch = await api.drain();
        if (batch.length) this.process(batch);
        this.tick++;
      } catch {
        // backend not ready yet; keep trying
      }
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }

  stopLoop() {
    this.running = false;
  }

  private process(batch: VoiceFrame[]) {
    const t = this.zones?.effective;
    const s = this.zones?.starting;
    // Per-frame bipolar progress (null when the metric is absent or no zones).
    const pProg = (f: VoiceFrame) =>
      t && s && f.f0 != null ? progress1d(f.f0, s.pitch_lo, s.pitch_hi, t.pitch_lo, t.pitch_hi) : null;
    const fProg = (f: VoiceFrame) =>
      t && s && f.f1 != null && f.f2 != null ? progress2d(f.f1, f.f2, s, t) : null;
    const wProg = (f: VoiceFrame) =>
      t && s && f.weight != null ? progress1d(f.weight, s.weight_lo, s.weight_hi, t.weight_lo, t.weight_hi) : null;

    for (const raw of batch) {
      // Smooth (keep EMA state through gaps).
      this.sm.f0 = ema(this.sm.f0, raw.f0);
      this.sm.f1 = ema(this.sm.f1, raw.f1);
      this.sm.f2 = ema(this.sm.f2, raw.f2);
      this.sm.weight = ema(this.sm.weight, raw.weight);
      this.sm.rms += SMOOTH * (raw.rms - this.sm.rms);
      const f: VoiceFrame = {
        timestamp_ms: raw.timestamp_ms,
        f0: raw.f0 == null ? null : this.sm.f0,
        f1: raw.f1 == null ? null : this.sm.f1,
        f2: raw.f2 == null ? null : this.sm.f2,
        weight: raw.weight == null ? null : this.sm.weight,
        rms: this.sm.rms,
      };

      if (this.sessionActive && t) {
        this.add(this.sPitch, inBand(f.f0, t.pitch_lo, t.pitch_hi));
        this.add(this.sFmt, fmtInBand(f, t));
        this.add(this.sWeight, inBand(f.weight, t.weight_lo, t.weight_hi));
        accProg(this.pPitch, pProg(f));
        accProg(this.pFmt, fProg(f));
        accProg(this.pWeight, wProg(f));
        this.sLast = f.timestamp_ms;
      }
      this.frames.push(f);
    }

    const now = this.frames[this.frames.length - 1]?.timestamp_ms ?? 0;

    // Session-trend buckets.
    while (this.sessionActive && now >= this.sNextBucket) {
      const lo = Math.max(0, this.sNextBucket - BUCKET_MS);
      const hi = this.sNextBucket;
      const win = this.frames.filter((f) => f.timestamp_ms >= lo && f.timestamp_ms < hi);
      if (win.length) {
        this.trend.push({
          t: (hi - this.sStart) / 1000,
          f0: median(win.map((f) => f.f0)),
          f1: median(win.map((f) => f.f1)),
          f2: median(win.map((f) => f.f2)),
          weight: median(win.map((f) => f.weight)),
        });
      }
      this.sNextBucket += BUCKET_MS;
    }

    // Evict old frames.
    const cutoff = now - HISTORY_MS;
    let drop = 0;
    while (drop < this.frames.length && this.frames[drop].timestamp_ms < cutoff) drop++;
    if (drop) this.frames.splice(0, drop);

    // Recompute reactive snapshots.
    this.current = this.frames.length ? this.frames[this.frames.length - 1] : null;
    if (t) {
      this.pitchCell = this.cell((f) => inBand(f.f0, t.pitch_lo, t.pitch_hi), this.sPitch);
      this.fmtCell = this.cell((f) => fmtInBand(f, t), this.sFmt);
      this.weightCell = this.cell((f) => inBand(f.weight, t.weight_lo, t.weight_hi), this.sWeight);
      this.pitchProg = this.progCell(pProg, this.pPitch);
      this.fmtProg = this.progCell(fProg, this.pFmt);
      this.weightProg = this.progCell(wProg, this.pWeight);
    }
  }

  private progCell(fn: (f: VoiceFrame) => number | null, sess: { sum: number; n: number }): Cell {
    return {
      now: nowNum(this.frames, fn),
      w5: avgNum(this.frames, 5_000, fn),
      w30: avgNum(this.frames, 30_000, fn),
      session: sess.n > 0 ? sess.sum / sess.n : null,
    };
  }

  private add(c: Counts, m: boolean | null) {
    if (m == null) return;
    c.total++;
    if (m) c.inb++;
  }

  private cell(pred: (f: VoiceFrame) => boolean | null, sess: Counts): Cell {
    return {
      now: nowVal(this.frames, pred),
      w5: frac(this.frames, 5_000, pred),
      w30: frac(this.frames, 30_000, pred),
      session: sess.total > 0 ? sess.inb / sess.total : null,
    };
  }

  // --- chart data getters (last N seconds) ---
  pitchRibbon(): [number, number | null][] {
    const now = this.frames[this.frames.length - 1]?.timestamp_ms ?? 0;
    const lo = now - RIBBON_MS;
    return this.frames
      .filter((f) => f.timestamp_ms >= lo)
      .map((f) => [f.timestamp_ms / 1000, f.f0] as [number, number | null]);
  }
  ribbonWindow(): [number, number] {
    const now = (this.frames[this.frames.length - 1]?.timestamp_ms ?? 0) / 1000;
    return [now - RIBBON_MS / 1000, now];
  }
  trendSeries(key: "f0" | "f1" | "f2" | "weight"): [number, number | null][] {
    return this.trend.map((b) => [b.t, b[key]]);
  }
  trendMaxX(): number {
    return Math.max(10, this.trend.length ? this.trend[this.trend.length - 1].t : 10);
  }
  formantCloud(): [number, number][] {
    const now = this.frames[this.frames.length - 1]?.timestamp_ms ?? 0;
    const lo = now - CLOUD_MS;
    return this.frames
      .filter((f) => f.timestamp_ms >= lo && f.f1 != null && f.f2 != null)
      .map((f) => [f.f1 as number, f.f2 as number]);
  }

  // --- session control ---
  startSession() {
    this.sessionActive = true;
    this.summary = null;
    this.trend = [];
    this.sPitch = { inb: 0, total: 0 };
    this.sFmt = { inb: 0, total: 0 };
    this.sWeight = { inb: 0, total: 0 };
    this.pPitch = { sum: 0, n: 0 };
    this.pFmt = { sum: 0, n: 0 };
    this.pWeight = { sum: 0, n: 0 };
    const now = this.frames[this.frames.length - 1]?.timestamp_ms ?? 0;
    this.sStart = now;
    this.sLast = now;
    this.sNextBucket = now + BUCKET_MS;
  }
  stopSession() {
    this.sessionActive = false;
    if (this.sPitch.total > 0 || this.trend.length) {
      this.summary = {
        durationS: (this.sLast - this.sStart) / 1000,
        medianPitch: median(this.trend.map((b) => b.f0)),
        pitch: this.sPitch.total ? this.sPitch.inb / this.sPitch.total : null,
        fmt: this.sFmt.total ? this.sFmt.inb / this.sFmt.total : null,
        weight: this.sWeight.total ? this.sWeight.inb / this.sWeight.total : null,
      };
    }
  }

  // --- capture + settings ---
  async start(device: string | null) {
    this.frames = [];
    this.trend = [];
    this.sessionActive = false;
    this.summary = null;
    this.sm = { f0: null, f1: null, f2: null, weight: null, rms: 0 };
    try {
      this.status = await api.startCapture(device);
    } catch (e) {
      this.status = { listening: false, device_name: "", sample_rate: 0, lost: false };
      console.error(e);
    }
  }
  async stop() {
    await api.stopCapture();
    this.status = await api.status();
  }
  async saveSettings(s: Settings) {
    this.settings = s;
    await api.saveSettings(s);
    this.zones = await api.zones();
  }
}

function inBand(v: number | null, lo: number, hi: number): boolean | null {
  return v == null ? null : v >= lo && v <= hi;
}
function fmtInBand(f: VoiceFrame, t: { f1_lo: number; f1_hi: number; f2_lo: number; f2_hi: number }): boolean | null {
  if (f.f1 == null || f.f2 == null) return null;
  return f.f1 >= t.f1_lo && f.f1 <= t.f1_hi && f.f2 >= t.f2_lo && f.f2 <= t.f2_hi;
}
function nowVal(frames: VoiceFrame[], pred: (f: VoiceFrame) => boolean | null): number | null {
  for (let i = frames.length - 1; i >= 0; i--) {
    const m = pred(frames[i]);
    if (m != null) return m ? 1 : 0;
  }
  return null;
}
function frac(frames: VoiceFrame[], windowMs: number, pred: (f: VoiceFrame) => boolean | null): number | null {
  const now = frames[frames.length - 1]?.timestamp_ms ?? 0;
  const cutoff = now - windowMs;
  let total = 0, inb = 0;
  for (let i = frames.length - 1; i >= 0; i--) {
    if (frames[i].timestamp_ms < cutoff) break;
    const m = pred(frames[i]);
    if (m != null) { total++; if (m) inb++; }
  }
  return total > 0 ? inb / total : null;
}
// Bipolar progress: -1 at the starting band center, +1 at the goal band center.
function progress1d(v: number, sLo: number, sHi: number, gLo: number, gHi: number): number {
  const sC = (sLo + sHi) / 2;
  const gC = (gLo + gHi) / 2;
  const denom = gC - sC;
  if (Math.abs(denom) < 1e-6) return 0;
  const t = (v - sC) / denom;
  return Math.max(-1, Math.min(1, 2 * t - 1));
}
function progress2d(
  v1: number,
  v2: number,
  s: { f1_lo: number; f1_hi: number; f2_lo: number; f2_hi: number },
  g: { f1_lo: number; f1_hi: number; f2_lo: number; f2_hi: number },
): number {
  const s1 = (s.f1_lo + s.f1_hi) / 2, s2 = (s.f2_lo + s.f2_hi) / 2;
  const g1 = (g.f1_lo + g.f1_hi) / 2, g2 = (g.f2_lo + g.f2_hi) / 2;
  const dx = g1 - s1, dy = g2 - s2;
  const denom = dx * dx + dy * dy;
  if (denom < 1e-6) return 0;
  const t = ((v1 - s1) * dx + (v2 - s2) * dy) / denom;
  return Math.max(-1, Math.min(1, 2 * t - 1));
}
function accProg(acc: { sum: number; n: number }, v: number | null) {
  if (v == null) return;
  acc.sum += v;
  acc.n++;
}
function nowNum(frames: VoiceFrame[], fn: (f: VoiceFrame) => number | null): number | null {
  for (let i = frames.length - 1; i >= 0; i--) {
    const v = fn(frames[i]);
    if (v != null) return v;
  }
  return null;
}
function avgNum(frames: VoiceFrame[], windowMs: number, fn: (f: VoiceFrame) => number | null): number | null {
  const now = frames[frames.length - 1]?.timestamp_ms ?? 0;
  const cutoff = now - windowMs;
  let sum = 0, n = 0;
  for (let i = frames.length - 1; i >= 0; i--) {
    if (frames[i].timestamp_ms < cutoff) break;
    const v = fn(frames[i]);
    if (v != null) { sum += v; n++; }
  }
  return n > 0 ? sum / n : null;
}

function median(xs: (number | null)[]): number | null {
  const v = xs.filter((x): x is number => x != null).sort((a, b) => a - b);
  return v.length ? v[Math.floor(v.length / 2)] : null;
}

export const store = new AppStore();
