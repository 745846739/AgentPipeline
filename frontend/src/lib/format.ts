/** 通用格式化辅助。 */

/** 工具调用参数摘要：`path="design.md", limit=20`（过长截断）。 */
export function summarizeArgs(argsJson: string, maxLen = 80): string {
  if (!argsJson) return '';
  let value: unknown;
  try {
    value = JSON.parse(argsJson);
  } catch {
    return truncate(argsJson, maxLen);
  }
  if (value && typeof value === 'object') {
    const parts = Object.entries(value as Record<string, unknown>).map(([k, v]) => {
      const rendered = typeof v === 'string' ? `"${v}"` : JSON.stringify(v);
      return `${k}=${rendered}`;
    });
    return truncate(parts.join(', '), maxLen);
  }
  return truncate(String(value), maxLen);
}

export function truncate(s: string, maxLen: number): string {
  if (s.length <= maxLen) return s;
  return `${s.slice(0, maxLen - 1)}…`;
}

export function lineCount(s: string | null | undefined): number {
  if (!s) return 0;
  return s.split('\n').length;
}

export function formatClock(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleTimeString('zh-CN', { hour12: false });
}

export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleString('zh-CN', { hour12: false });
}

export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}
