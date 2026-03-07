import type { TimeRange } from "../hooks/useStats";

export function formatTimestamp(iso: string, timeRange: TimeRange): string {
  const d = new Date(iso);
  if (timeRange === "30d") {
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }
  if (timeRange === "7d") {
    return d.toLocaleDateString("en-US", { weekday: "short", hour: "numeric" });
  }
  return d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
}

export function formatUSD(value: number): string {
  if (value >= 1000) return `$${value.toLocaleString("en-US", { maximumFractionDigits: 0 })}`;
  if (value >= 1) return `$${value.toFixed(2)}`;
  return `$${value.toFixed(4)}`;
}

export function formatMs(ms: number): string {
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)}m`;
  if (ms >= 1_000) return `${(ms / 1_000).toFixed(1)}s`;
  return `${ms.toFixed(0)}ms`;
}

export function truncateHash(hash: string, startLen = 8, endLen = 4): string {
  if (hash.length <= startLen + endLen) return hash;
  return `${hash.slice(0, startLen)}\u2026${hash.slice(-endLen)}`;
}
