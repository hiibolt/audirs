<script lang="ts">
  import { store } from "$lib/store.svelte";
  import { redGreen, neutralGreen, neutralBlue } from "$lib/theme";

  // Bipolar (from-the-middle) bars when the starting zone is shown; otherwise
  // unipolar fraction bars. The "now" column is always instant (full = in band).
  const bipolar = $derived(store.settings?.show_starting ?? false);

  const cols = ["now", "w5", "w30", "session"] as const;
  const head: Record<string, string> = { now: "now", w5: "5s", w30: "30s", session: "session" };

  const metrics = $derived([
    { label: "Pitch", cell: store.pitchCell, prog: store.pitchProg },
    { label: "Formants", cell: store.fmtCell, prog: store.fmtProg },
    { label: "Weight", cell: store.weightCell, prog: store.weightProg },
  ]);
</script>

{#snippet bar(col: (typeof cols)[number], cell: any, prog: any)}
  {@const useBipolar = bipolar && col !== "now"}
  <div class="track">
    {#if useBipolar}
      <div class="mid"></div>
      {#if prog[col] != null}
        {@const p = Math.max(-1, Math.min(1, prog[col]))}
        {#if p >= 0}
          <div class="fill" style="bottom:50%; height:{p * 50}%; background:{neutralGreen(p)}"></div>
        {:else}
          <div class="fill" style="top:50%; height:{-p * 50}%; background:{neutralBlue(-p)}"></div>
        {/if}
        <div class="cursor" style="top:{50 * (1 - p)}%"></div>
      {/if}
    {:else}
      {@const f = cell[col] as number | null}
      {#if f != null && f > 0}
        <div class="fill" style="bottom:0; height:{f * 100}%; background:{redGreen(f)}"></div>
      {/if}
    {/if}
  </div>
{/snippet}

<div class="grid">
  <span></span>
  {#each cols as c}<span class="h">{head[c]}</span>{/each}

  {#each metrics as m}
    <span class="rl">{m.label}</span>
    {#each cols as c}{@render bar(c, m.cell, m.prog)}{/each}
  {/each}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: 86px repeat(4, minmax(0, 1fr));
    column-gap: 10px;
    row-gap: 10px;
    align-items: center;
  }
  .h { font-size: 0.72rem; text-align: center; color: var(--ink); opacity: 0.7; }
  .rl { font-size: 0.84rem; font-weight: 600; color: var(--ink); }
  .track {
    position: relative; overflow: hidden; border-radius: 5px;
    background: var(--track); height: 48px;
  }
  .fill { position: absolute; left: 0; width: 100%; transition: height 0.12s ease, background 0.12s ease; }
  .mid { position: absolute; left: 0; right: 0; top: 50%; height: 1px; background: rgba(70, 52, 63, 0.28); }
  .cursor { position: absolute; left: -1px; right: -1px; height: 2px; background: var(--ink); transform: translateY(-1px); transition: top 0.12s ease; }
</style>
