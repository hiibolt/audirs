<script lang="ts">
  import * as echarts from "echarts";
  import { onMount } from "svelte";

  let { option, height = "240px" }: { option: any; height?: string } = $props();

  let el: HTMLDivElement;
  let chart: echarts.ECharts | undefined;

  onMount(() => {
    chart = echarts.init(el, undefined, { renderer: "canvas" });
    const ro = new ResizeObserver(() => chart?.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      chart?.dispose();
    };
  });

  $effect(() => {
    if (chart && option) chart.setOption(option, { notMerge: true, lazyUpdate: true });
  });
</script>

<div bind:this={el} style="width:100%; height:{height};"></div>
