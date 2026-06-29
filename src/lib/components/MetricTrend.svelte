<script lang="ts">
  import EChart from "./EChart.svelte";
  import { COLORS } from "$lib/theme";

  let {
    title,
    series,
    yMin,
    yMax,
    maxX,
    bands = [],
    unit = "",
  }: {
    title: string;
    series: { data: [number, number | null][]; color: string }[];
    yMin: number;
    yMax: number;
    maxX: number;
    bands?: { lo: number; hi: number; color: string }[];
    unit?: string;
  } = $props();

  const option = $derived.by(() => {
    const areas = bands.map((b) => [
      { yAxis: b.lo, itemStyle: { color: b.color } },
      { yAxis: b.hi },
    ]);
    return {
      animation: false,
      grid: { left: 48, right: 14, top: 10, bottom: 28 },
      xAxis: { type: "value", min: 0, max: maxX, name: "seconds", nameLocation: "middle", nameGap: 20, axisLabel: { color: COLORS.ink } },
      yAxis: { type: "value", min: yMin, max: yMax, name: unit, axisLabel: { color: COLORS.ink }, splitLine: { lineStyle: { color: "#f0f0f0" } } },
      series: series.map((s, i) => ({
        type: "line",
        data: s.data,
        showSymbol: false,
        smooth: true,
        connectNulls: false,
        lineStyle: { color: s.color, width: 2.5 },
        z: 5,
        markArea: i === 0 ? { silent: true, data: areas } : undefined,
      })),
    };
  });
</script>

<div class="mt">
  <div class="t">{title}</div>
  <EChart {option} height="168px" />
</div>

<style>
  .mt { display: flex; flex-direction: column; }
  .t { color: var(--accent); font-weight: 700; font-size: 0.9rem; margin-bottom: 2px; }
</style>
