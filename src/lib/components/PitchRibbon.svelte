<script lang="ts">
  import EChart from "./EChart.svelte";
  import { store } from "$lib/store.svelte";
  import { COLORS } from "$lib/theme";

  const option = $derived.by(() => {
    store.tick; // re-run each frame
    const z = store.zones?.effective;
    const start = store.zones?.starting;
    const showStart = store.settings?.show_starting;
    const [x0, x1] = store.ribbonWindow();
    const data = store.pitchRibbon();

    const areas: any[] = [
      [{ yAxis: 80, itemStyle: { color: COLORS.outFill } }, { yAxis: 350 }],
    ];
    if (showStart && start)
      areas.push([
        { yAxis: start.pitch_lo, itemStyle: { color: COLORS.startFill } },
        { yAxis: start.pitch_hi },
      ]);
    if (z)
      areas.push([
        { yAxis: z.pitch_lo, itemStyle: { color: COLORS.zoneFill } },
        { yAxis: z.pitch_hi },
      ]);

    return {
      animation: false,
      grid: { left: 44, right: 14, top: 12, bottom: 22 },
      xAxis: { type: "value", min: x0, max: x1, axisLabel: { show: false }, splitLine: { show: false }, axisLine: { lineStyle: { color: "#ddd" } } },
      yAxis: { type: "value", min: 80, max: 350, name: "Hz", nameTextStyle: { color: COLORS.ink }, axisLabel: { color: COLORS.ink }, splitLine: { lineStyle: { color: "#f0f0f0" } } },
      series: [
        {
          type: "line",
          data,
          showSymbol: false,
          connectNulls: false,
          lineStyle: { color: COLORS.accent, width: 2.5 },
          z: 5,
          markArea: { silent: true, data: areas },
        },
      ],
    };
  });
</script>

<EChart {option} height="240px" />
