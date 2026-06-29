<script lang="ts">
  import { store } from "$lib/store.svelte";
  import { COLORS } from "$lib/theme";

  const MIN = -5, MAX = 22;
  const pos = (v: number) => ((v - MIN) / (MAX - MIN)) * 100;

  const z = $derived(store.zones?.effective);
  const start = $derived(store.zones?.starting);
  const showStart = $derived(store.settings?.show_starting ?? false);
  const w = $derived(store.current?.weight ?? null);
</script>

<div class="bar">
  {#if showStart && start}
    <div class="band start" style="left:{pos(start.weight_lo)}%; width:{pos(start.weight_hi) - pos(start.weight_lo)}%"></div>
  {/if}
  {#if z}
    <div class="band goal" style="left:{pos(z.weight_lo)}%; width:{pos(z.weight_hi) - pos(z.weight_lo)}%"></div>
  {/if}
  {#if w != null}
    <div class="marker" style="left:{pos(w)}%"></div>
  {/if}
</div>
<div class="caption">lighter ◂ vocal weight (H1–H2) ▸ heavier</div>

<style>
  .bar { position: relative; height: 30px; border-radius: 7px; background: var(--track); overflow: hidden; }
  .band { position: absolute; top: 0; bottom: 0; }
  .goal { background: rgba(150, 220, 165, 0.55); }
  .start { background: rgba(110, 160, 210, 0.3); }
  .marker { position: absolute; top: -3px; bottom: -3px; width: 3px; background: var(--accent); transform: translateX(-50%); transition: left 0.12s ease; }
  .caption { color: var(--ink); font-size: 0.72rem; opacity: 0.7; margin-top: 4px; text-align: center; }
</style>
