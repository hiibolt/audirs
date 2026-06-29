// Shared palette — calm, accessible. Light-green target zones, soft-red
// out-of-zone, pink accents, light-blue starting overlay. Matches the PoC.
export const COLORS = {
  ink: "#46343f",
  accent: "#eb6eaf", // pink
  zoneFill: "rgba(150, 220, 165, 0.45)", // light green
  zoneLine: "#5fb478",
  outFill: "rgba(235, 150, 150, 0.18)", // soft red
  outLine: "#d26e6e",
  startFill: "rgba(110, 160, 210, 0.22)", // light blue overlay
  startLine: "#6ea0d2",
  track: "#ececf0",
  bg: "#faf8fb",
  card: "#ffffff",
};

// Red (0) -> green (1), for the unipolar in-band fill bars.
export function redGreen(f: number): string {
  const t = Math.max(0, Math.min(1, f));
  const r = Math.round(40 + (1 - t) * 200);
  const g = Math.round(40 + t * 180);
  return `rgb(${r}, ${g}, 60)`;
}

// Neutral track gray -> green (toward/past the goal zone), f in [0, 1].
export function neutralGreen(f: number): string {
  const t = Math.max(0, Math.min(1, f));
  return `rgb(${Math.round(210 + (95 - 210) * t)}, ${Math.round(210 + (180 - 210) * t)}, ${Math.round(210 + (120 - 210) * t)})`;
}

// Neutral track gray -> starting-zone blue (toward/past the start), f in [0, 1].
export function neutralBlue(f: number): string {
  const t = Math.max(0, Math.min(1, f));
  return `rgb(${Math.round(210 + (110 - 210) * t)}, ${Math.round(210 + (160 - 210) * t)}, 210)`;
}
