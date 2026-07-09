import { formatBytes } from "../lib/format";

export interface BarItem {
  label: string;
  value: number;
  display?: string;
  color?: "accent" | "green" | "yellow" | "gray";
}

export function BarChart({ items }: { items: BarItem[] }) {
  const max = Math.max(1, ...items.map((i) => i.value));
  return (
    <div>
      {items.map((it, i) => (
        <div className="bar-row" key={i}>
          <div className="bar-label" title={it.label}>
            {it.label}
          </div>
          <div className="bar-track">
            <div
              className={"bar-fill " + (it.color ?? "accent")}
              style={{ width: `${Math.max(2, (it.value / max) * 100)}%` }}
            />
          </div>
          <div className="bar-value">{it.display ?? formatBytes(it.value)}</div>
        </div>
      ))}
    </div>
  );
}

// A reuse/change heatmap synthesized from per-file reuse ratios.
export function ReuseHeatmap({
  segments,
}: {
  segments: { ratio: number; width: number; label?: string }[];
}) {
  const total = Math.max(1, segments.reduce((a, s) => a + s.width, 0));
  const colorFor = (ratio: number) => {
    if (ratio >= 0.85) return "var(--green)";
    if (ratio >= 0.5) return "var(--yellow)";
    if (ratio >= 0.15) return "var(--red)";
    return "var(--gray)";
  };
  return (
    <div>
      <div className="heat">
        {segments.map((s, i) => (
          <span
            key={i}
            title={`${s.label ?? ""} ${(s.ratio * 100).toFixed(0)}% reused`}
            style={{ width: `${(s.width / total) * 100}%`, background: colorFor(s.ratio) }}
          />
        ))}
      </div>
      <div className="heat-legend">
        <span><i style={{ background: "var(--green)" }} />reused</span>
        <span><i style={{ background: "var(--yellow)" }} />changed</span>
        <span><i style={{ background: "var(--red)" }} />scattered</span>
        <span><i style={{ background: "var(--gray)" }} />high entropy</span>
      </div>
    </div>
  );
}

export function Donut({ percent, label }: { percent: number; label: string }) {
  const r = 34;
  const c = 2 * Math.PI * r;
  const p = Math.max(0, Math.min(100, percent));
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
      <svg width="86" height="86" viewBox="0 0 86 86">
        <circle cx="43" cy="43" r={r} fill="none" stroke="var(--bg-elev-2)" strokeWidth="9" />
        <circle
          cx="43"
          cy="43"
          r={r}
          fill="none"
          stroke="var(--green)"
          strokeWidth="9"
          strokeLinecap="round"
          strokeDasharray={`${(p / 100) * c} ${c}`}
          transform="rotate(-90 43 43)"
        />
        <text x="43" y="48" textAnchor="middle" fontSize="17" fontWeight="700" fill="var(--text)">
          {p.toFixed(0)}%
        </text>
      </svg>
      <div className="text-dim" style={{ fontSize: 12.5 }}>{label}</div>
    </div>
  );
}
