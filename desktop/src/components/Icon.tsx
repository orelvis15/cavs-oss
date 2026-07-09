// Compact inline SVG icon set (stroke-based, 24x24 viewbox).
import type { CSSProperties } from "react";

const PATHS: Record<string, string> = {
  home: "M3 10.5 12 3l9 7.5M5 9.5V21h14V9.5",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  "folder-open": "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2M3 7v11a2 2 0 0 0 2 2h13.5a1.5 1.5 0 0 0 1.45-1.1L22 10H6.2a2 2 0 0 0-1.9 1.37z",
  rocket: "M5 15c-1.5 1.5-2 5-2 5s3.5-.5 5-2M9 11a10 10 0 0 1 8-8c1.5 0 2 .5 2 2a10 10 0 0 1-8 8zM9 11l-3 1 1 3 3-1M14 9a1 1 0 1 0 0-.01",
  plug: "M9 3v5m6-5v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4",
  package: "M12 3 3 7.5v9L12 21l9-4.5v-9zM3 7.5 12 12l9-4.5M12 12v9",
  chart: "M4 20V6m5 14V10m5 10V4m5 16v-8",
  layers: "M12 3 2 8.5 12 14l10-5.5zM2 13.5 12 19l10-5.5M2 18 12 23l10-5",
  columns: "M4 4h16v16H4zM12 4v16",
  coins: "M8 8a5 3 0 1 0 0-.01M3 8v4c0 1.7 2.2 3 5 3s5-1.3 5-3V8M13 12a5 3 0 1 0 8 0 5 3 0 0 0-8 0zM13 12v4c0 1.7 2.2 3 5 3",
  sparkles: "M12 3l1.8 4.7L18.5 9.5 13.8 11.3 12 16l-1.8-4.7L5.5 9.5l4.7-1.8zM19 15l.8 2 2 .8-2 .8-.8 2-.8-2-2-.8 2-.8z",
  check: "M4 12.5 9 17.5 20 6.5",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM16 16l4.5 4.5",
  eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
  route: "M6 19a2 2 0 1 0 0-.01M18 5a2 2 0 1 0 0-.01M8 5h6a4 4 0 0 1 0 8H10a4 4 0 0 0 0 8h6",
  gauge: "M4 18a8 8 0 1 1 16 0M12 14l4-4",
  grid: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
  download: "M12 3v11m0 0 4-4m-4 4-4-4M4 19h16",
  share: "M6 12a2 2 0 1 0 0-.01M18 6a2 2 0 1 0 0-.01M18 18a2 2 0 1 0 0-.01M8 11l8-4M8 13l8 4",
  history: "M3 12a9 9 0 1 0 3-6.7M3 4v4h4M12 7v5l3 2",
  server: "M4 5h16v5H4zM4 14h16v5H4zM8 7.5h.01M8 16.5h.01",
  code: "M9 8l-5 4 5 4M15 8l5 4-5 4",
  terminal: "M4 5h16v14H4zM7 9l3 2.5L7 14M12.5 14h4",
  report: "M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6",
  bulb: "M9 18h6M10 21h4M8 11a4 4 0 1 1 8 0c0 1.7-1 2.5-1.5 3.5-.3.6-.5 1-.5 1.5h-4c0-.5-.2-.9-.5-1.5C9 13.5 8 12.7 8 11z",
  cube: "M12 3 3 7.5v9L12 21l9-4.5v-9zM3 7.5 12 12l9-4.5M12 12v9",
  filter: "M3 5h18l-7 8v6l-4-2v-4z",
  shield: "M12 3 5 6v5c0 4.5 3 7.5 7 10 4-2.5 7-5.5 7-10V6zM9 12l2 2 4-4",
  database: "M12 3c4.4 0 8 1.3 8 3s-3.6 3-8 3-8-1.3-8-3 3.6-3 8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  export: "M12 14V3m0 0 4 4m-4-4-4 4M5 13v6h14v-6",
  book: "M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2zM19 3v16",
  list: "M8 6h13M8 12h13M8 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01",
  chat: "M4 5h16v11H9l-4 4v-4H4zM8 9h8M8 12h5",
  gear: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM19.4 13a7.8 7.8 0 0 0 0-2l2-1.5-2-3.5-2.3 1a7.6 7.6 0 0 0-1.7-1l-.4-2.5h-4l-.4 2.5a7.6 7.6 0 0 0-1.7 1l-2.3-1-2 3.5L4.6 11a7.8 7.8 0 0 0 0 2l-2 1.5 2 3.5 2.3-1a7.6 7.6 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7.6 7.6 0 0 0 1.7-1l2.3 1 2-3.5z",
  // UI
  help: "M9 9a3 3 0 1 1 4 2.8c-.8.4-1 1-1 2M12 17h.01M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
  close: "M6 6l12 12M18 6 6 18",
  copy: "M9 9h10v11H9zM5 15V4h10",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
  info: "M12 8h.01M11 12h1v5h1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
  plus: "M12 5v14M5 12h14",
  external: "M14 4h6v6M20 4l-8 8M18 14v5H5V6h5",
  play: "M7 5l12 7-12 7z",
  stop: "M6 6h12v12H6z",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2v2M12 20v2M4 12H2M22 12h-2M5 5l1.5 1.5M17.5 17.5 19 19M19 5l-1.5 1.5M6.5 17.5 5 19",
  moon: "M20 14a8 8 0 1 1-10-10 7 7 0 0 0 10 10z",
  globe: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c2.5 2.5 3.5 6 3.5 9S14.5 18.5 12 21c-2.5-2.5-3.5-6-3.5-9S9.5 5.5 12 3z",
  chevron: "M9 6l6 6-6 6",
  refresh: "M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5",
};

export function Icon({
  name,
  className,
  size,
  style,
}: {
  name: string;
  className?: string;
  size?: number;
  style?: CSSProperties;
}) {
  const d = PATHS[name] ?? PATHS.info;
  const s = size ?? 18;
  return (
    <svg
      className={className}
      style={style}
      width={s}
      height={s}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {d.split("M").filter(Boolean).map((seg, i) => (
        <path key={i} d={"M" + seg} />
      ))}
    </svg>
  );
}
