<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "$lib/store.svelte";
  import { COLORS } from "$lib/theme";
  import type { Gender, Settings } from "$lib/api";
  import PitchRibbon from "$lib/components/PitchRibbon.svelte";
  import FormantScatter from "$lib/components/FormantScatter.svelte";
  import WeightBar from "$lib/components/WeightBar.svelte";
  import InBandGrid from "$lib/components/InBandGrid.svelte";
  import MetricTrend from "$lib/components/MetricTrend.svelte";

  type Tab = "live" | "session" | "settings";
  let tab = $state<Tab>("live");
  const tabs: { id: Tab; label: string }[] = [
    { id: "live", label: "Live" },
    { id: "session", label: "Session" },
    { id: "settings", label: "Settings" },
  ];

  onMount(async () => {
    await store.init();
    store.startLoop();
    await store.start(store.settings?.device ?? null);
  });

  function patch(p: Partial<Settings>) {
    if (!store.settings) return;
    store.saveSettings({ ...store.settings, ...p });
  }
  async function pickDevice(name: string) {
    const device = name === "__default__" ? null : name;
    patch({ device });
    await store.start(device);
  }
  function fmtPct(v: number | null): string {
    return v == null ? "—" : `${Math.round(v * 100)}%`;
  }

  const lost = $derived(store.status.lost || !store.status.listening);

  // --- session trend props (recompute each tick) ---
  const zEff = $derived(store.zones?.effective);
  const zStart = $derived(store.zones?.starting);
  const showStart = $derived(store.settings?.show_starting ?? false);
  const maxX = $derived((store.tick, store.trendMaxX()));

  const pitchSeries = $derived((store.tick, [{ data: store.trendSeries("f0"), color: COLORS.accent }]));
  const weightSeries = $derived((store.tick, [{ data: store.trendSeries("weight"), color: COLORS.accent }]));
  const formantSeries = $derived(
    (store.tick, [
      { data: store.trendSeries("f1"), color: "#8e6fd1" },
      { data: store.trendSeries("f2"), color: COLORS.accent },
    ]),
  );
  function gband(lo: number, hi: number) { return { lo, hi, color: COLORS.zoneFill }; }
  function sband(lo: number, hi: number) { return { lo, hi, color: COLORS.startFill }; }
  const pitchBands = $derived([
    ...(showStart && zStart ? [sband(zStart.pitch_lo, zStart.pitch_hi)] : []),
    ...(zEff ? [gband(zEff.pitch_lo, zEff.pitch_hi)] : []),
  ]);
  const weightBands = $derived([
    ...(showStart && zStart ? [sband(zStart.weight_lo, zStart.weight_hi)] : []),
    ...(zEff ? [gband(zEff.weight_lo, zEff.weight_hi)] : []),
  ]);
  const formantBands = $derived([
    ...(showStart && zStart ? [sband(zStart.f1_lo, zStart.f1_hi), sband(zStart.f2_lo, zStart.f2_hi)] : []),
    ...(zEff ? [gband(zEff.f1_lo, zEff.f1_hi), gband(zEff.f2_lo, zEff.f2_hi)] : []),
  ]);
</script>

<div class="app">
  <header>
    <div class="title">Audirs</div>
    <nav>
      {#each tabs as t}
        <button class:active={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>
      {/each}
    </nav>
    {#if store.sessionActive}
      <button class="primary" onclick={() => store.stopSession()}>■ Stop session</button>
    {:else}
      <button class="primary" onclick={() => store.startSession()}>▶ Start session</button>
    {/if}
    <div class="status">
      {#if store.status.listening}
        <span class="dot ok"></span>{store.status.device_name} · {(store.status.sample_rate / 1000).toFixed(1)} kHz
      {:else}
        <span class="dot bad"></span>not listening
      {/if}
      {#if lost}
        <button onclick={() => store.start(store.settings?.device ?? null)}>Reconnect</button>
      {/if}
    </div>
  </header>

  {#if tab === "live"}
    <div class="live">
      <div class="card">
        <h2>Pitch (F0)</h2>
        <PitchRibbon />
      </div>

      <div class="cols">
        <div class="card formants-card">
          <h2>Formants (F1 × F2)</h2>
          <div class="chartfill"><FormantScatter /></div>
        </div>
        <div class="rightcol">
          <div class="card">
            <h2>Vocal weight (H1–H2)</h2>
            <WeightBar />
          </div>
          <div class="card">
            <h2>In band</h2>
            <InBandGrid />
          </div>
        </div>
      </div>
    </div>
  {:else if tab === "session"}
    <div class="card">
      <h2>Session</h2>
      {#if store.summary}
        <p class="muted">
          Last session — {store.summary.durationS.toFixed(0)}s · median pitch
          {store.summary.medianPitch != null ? store.summary.medianPitch.toFixed(0) + " Hz" : "—"} ·
          in band: pitch {fmtPct(store.summary.pitch)}, formants {fmtPct(store.summary.fmt)},
          weight {fmtPct(store.summary.weight)}
        </p>
      {/if}
      {#if store.trend.length < 2}
        <p class="muted">Press “Start session” (top bar) and speak to build a trend across all three metrics.</p>
      {/if}
      <div class="trends">
        <MetricTrend title="Pitch (Hz)" series={pitchSeries} yMin={80} yMax={350} maxX={maxX} bands={pitchBands} unit="Hz" />
        <MetricTrend title="Formants — F1 (purple), F2 (pink)" series={formantSeries} yMin={0} yMax={3000} maxX={maxX} bands={formantBands} unit="Hz" />
        <MetricTrend title="Vocal weight (H1–H2, dB)" series={weightSeries} yMin={-5} yMax={22} maxX={maxX} bands={weightBands} unit="dB" />
      </div>
    </div>
  {:else}
    <div class="card settings">
      <h2>Settings</h2>
      {#if store.settings}
        <label>
          <span>Input device</span>
          <select onchange={(e) => pickDevice((e.target as HTMLSelectElement).value)}>
            <option value="__default__" selected={store.settings.device == null}>System default</option>
            {#each store.devices as d}
              <option value={d} selected={store.settings.device === d}>{d}</option>
            {/each}
          </select>
        </label>

        <label>
          <span>Mic boost — {store.settings.gain.toFixed(1)}×</span>
          <input type="range" min="1" max="30" step="0.5" value={store.settings.gain}
            oninput={(e) => patch({ gain: +(e.target as HTMLInputElement).value })} />
        </label>

        <label>
          <span>Silence threshold — {store.settings.threshold.toFixed(3)}</span>
          <input type="range" min="0" max="0.05" step="0.001" value={store.settings.threshold}
            oninput={(e) => patch({ threshold: +(e.target as HTMLInputElement).value })} />
        </label>

        <label>
          <span>Target voice</span>
          <select onchange={(e) => patch({ target_gender: (e.target as HTMLSelectElement).value as Gender })}>
            <option value="Female" selected={store.settings.target_gender === "Female"}>Feminine</option>
            <option value="Male" selected={store.settings.target_gender === "Male"}>Masculine</option>
          </select>
        </label>

        <label>
          <span>Goal — {Math.round(store.settings.goal_percent * 100)}% toward target</span>
          <input type="range" min="0" max="1" step="0.01" value={store.settings.goal_percent}
            oninput={(e) => patch({ goal_percent: +(e.target as HTMLInputElement).value })} />
        </label>

        <label class="check">
          <input type="checkbox" checked={store.settings.show_starting}
            onchange={(e) => patch({ show_starting: (e.target as HTMLInputElement).checked })} />
          <span>Show starting (opposite) zone for comparison</span>
        </label>
      {/if}
    </div>
  {/if}
</div>

<style>
  .app { max-width: 1000px; margin: 0 auto; padding: 14px 18px 28px; display: flex; flex-direction: column; gap: 14px; }
  header { display: flex; align-items: center; gap: 14px; flex-wrap: wrap; }
  .title { font-size: 1.25rem; font-weight: 800; color: var(--accent); }
  nav { display: flex; gap: 6px; }
  nav button.active { background: var(--accent); color: white; border-color: var(--accent); }
  .status { margin-left: auto; display: flex; align-items: center; gap: 8px; font-size: 0.82rem; color: var(--ink); }
  .dot { width: 9px; height: 9px; border-radius: 50%; display: inline-block; }
  .dot.ok { background: #5fb478; }
  .dot.bad { background: #d26e6e; }
  .live { display: flex; flex-direction: column; gap: 14px; }
  .cols { display: grid; grid-template-columns: 1.3fr 1fr; gap: 14px; align-items: stretch; }
  .formants-card { display: flex; flex-direction: column; }
  .chartfill { flex: 1; min-height: 260px; }
  .rightcol { display: flex; flex-direction: column; gap: 14px; }
  .trends { display: flex; flex-direction: column; gap: 18px; margin-top: 8px; }
  .settings { display: flex; flex-direction: column; gap: 16px; max-width: 520px; }
  .settings label { display: flex; flex-direction: column; gap: 6px; font-size: 0.88rem; }
  .settings label.check { flex-direction: row; align-items: center; gap: 9px; }
  .settings input[type="range"] { width: 100%; }
</style>
