<script lang="ts">
  import EChart from "./EChart.svelte";
  import { store } from "$lib/store.svelte";
  import { COLORS } from "$lib/theme";

  const X = [150, 1100];
  const Y = [1400, 3000];

  const option = $derived.by(() => {
    store.tick;
    const z = store.zones?.effective;
    const start = store.zones?.starting;
    const showStart = store.settings?.show_starting;
    const pts = store.formantCloud();
    const newest = pts.length ? [pts[pts.length - 1]] : [];

    const areas: any[] = [
      [{ xAxis: X[0], yAxis: Y[0], itemStyle: { color: COLORS.outFill } }, { xAxis: X[1], yAxis: Y[1] }],
    ];
    if (showStart && start)
      areas.push([
        { xAxis: start.f1_lo, yAxis: start.f2_lo, itemStyle: { color: COLORS.startFill } },
        { xAxis: start.f1_hi, yAxis: start.f2_hi },
      ]);
    if (z)
      areas.push([
        { xAxis: z.f1_lo, yAxis: z.f2_lo, itemStyle: { color: COLORS.zoneFill } },
        { xAxis: z.f1_hi, yAxis: z.f2_hi },
      ]);

    return {
      animation: false,
      grid: { left: 52, right: 16, top: 14, bottom: 36 },
      xAxis: { type: "value", min: X[0], max: X[1], name: "F1 (Hz)", nameLocation: "middle", nameGap: 22, axisLabel: { color: COLORS.ink }, splitLine: { show: false } },
      yAxis: { type: "value", min: Y[0], max: Y[1], name: "F2 (Hz)", axisLabel: { color: COLORS.ink }, splitLine: { lineStyle: { color: "#f3f3f3" } } },
      series: [
        {
          type: "scatter",
          data: pts,
          symbolSize: 6,
          itemStyle: { color: COLORS.accent, opacity: 0.35 },
          markArea: { silent: true, data: areas },
        },
        { type: "scatter", data: newest, symbolSize: 13, itemStyle: { color: COLORS.accent } },
      ],
    };
  });
</script>

<EChart {option} height="100%" />
