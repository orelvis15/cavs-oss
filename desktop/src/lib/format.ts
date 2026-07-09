export function formatBytes(n: number | null | undefined): string {
  if (n == null || isNaN(n)) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2)} ${units[i]}`;
}

export function formatDate(iso: string, lang: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(lang === "es" ? "es-ES" : "en-US", {
      dateStyle: "medium",
      timeStyle: "short",
    });
  } catch {
    return iso;
  }
}

export function formatPercent(ratio: number | null | undefined, digits = 1): string {
  if (ratio == null || isNaN(ratio)) return "—";
  return `${(ratio * 100).toFixed(digits)}%`;
}

export function basename(path: string): string {
  if (!path) return "";
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

export function truncateMiddle(s: string, max = 46): string {
  if (s.length <= max) return s;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return s.slice(0, head) + "…" + s.slice(s.length - tail);
}
