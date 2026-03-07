/**
 * Stable model color assignments. Regex matching so "claude-3-5-sonnet"
 * and "claude-3-opus" both resolve to violet without enumerating every variant.
 */
const MODEL_PALETTE: Array<[RegExp, string]> = [
  [/claude/i, "#7c3aed"],
  [/gpt|openai/i, "#16a34a"],
  [/gemini|google|gemma/i, "#2563eb"],
  [/mistral|ministral/i, "#d97706"],
  [/llama|meta/i, "#dc2626"],
  [/cohere/i, "#0891b2"],
  [/qwen/i, "#8b5cf6"],
  [/kimi|moonshot/i, "#ec4899"],
  [/minimax/i, "#f97316"],
  [/nova|amazon/i, "#14b8a6"],
];

const FALLBACK_COLORS = [
  "#6b7280", "#9333ea", "#0ea5e9", "#f59e0b", "#10b981", "#f43f5e",
];

const cache = new Map<string, string>();

function simpleHash(s: string): number {
  let hash = 0;
  for (let i = 0; i < s.length; i++) {
    hash = (hash * 31 + s.charCodeAt(i)) | 0;
  }
  return Math.abs(hash);
}

export function modelColor(modelName: string): string {
  const key = modelName.toLowerCase();
  if (cache.has(key)) return cache.get(key)!;

  for (const [pattern, color] of MODEL_PALETTE) {
    if (pattern.test(key)) {
      cache.set(key, color);
      return color;
    }
  }

  // Deterministic fallback based on name hash — stable across renders
  const color = FALLBACK_COLORS[simpleHash(key) % FALLBACK_COLORS.length];
  cache.set(key, color);
  return color;
}
