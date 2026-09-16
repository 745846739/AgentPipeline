/**
 * 市场来源编辑器的判据（决策 187）：归一 / 校验 / 增删。
 *
 * **后端是权威**：这里只做「按下按钮之前就能看见的错」——非法 origin、重复项。传输安全
 * （非回环必须 https）也在这里挡一次，理由同上：让用户点了保存才发现规则，不如在输入框旁
 * 直接说。规则本身与 `crates/core/src/config.rs::validate_market_sources` 同源，两处口径
 * 若漂移，表现是「前端说没问题、后端 400」——那正是这条注释存在的意义。
 */

/**
 * 归一化一个 origin：小写、去尾斜杠。
 *
 * **不做别的事**：与后端的 `normalize_origin` 逐字同口径（小写 + 去尾斜杠），刻意不顺手
 * 抹掉默认端口 `:443` —— 后端不抹，前端抹了就会出现「前端说这两项重复、后端收下两条」
 * 这种两处口径漂移。前端这份只是「按保存前的即时反馈」，后端始终是权威。
 */
export function normalizeSource(raw: string): string {
  let value = raw.trim();
  if (value === '') return '';
  value = value.toLowerCase();
  value = value.replace(/\/+$/, '');
  return value;
}

/**
 * 校验一条来源；返回面向用户的错误说明，`null` = 合法。
 *
 * 三条规则（与后端同源）：
 * - 必须是 origin：只有 `scheme://host[:port]`，不带路径 / 查询 / 片段；
 * - 只允许 http / https；
 * - 非回环必须 https（明文 http 上 `sha256` 挡不住同时替换索引与包的中间人，决策 177③）。
 */
export function validateSource(raw: string): string | null {
  const value = normalizeSource(raw);
  if (value === '') return '请填写来源地址。';
  const m = /^(https?):\/\/([^/?#]+)$/.exec(value);
  if (!m) {
    return '来源只能是 origin（scheme://host[:port]），不带路径、查询或片段。';
  }
  const scheme = m[1];
  const authority = m[2];
  if (authority === '' || /\s/.test(authority) || authority.includes('@')) {
    return 'origin 里的主机名不合法。';
  }
  // IPv6 字面量写作 [::1]:8787，取主机段时要把方括号里的整体当成一段
  const host = authority.startsWith('[')
    ? (authority.slice(1).split(']')[0] ?? '')
    : (authority.split(':')[0] ?? '');
  if (host === '') return 'origin 里没有主机名。';
  if (scheme === 'http') {
    const loopback = host === '127.0.0.1' || host === 'localhost' || host === '::1' || host.startsWith('127.');
    if (!loopback) {
      return '非回环来源必须用 https：明文 http 挡不住中间人（决策 177③）。';
    }
  }
  return null;
}

/** 追加一条来源；非法或重复 → `null`（调用方给出提示），合法 → 新列表。 */
export function addSource(sources: string[], raw: string): string[] | null {
  const value = normalizeSource(raw);
  if (validateSource(value) !== null || sources.includes(value)) return null;
  return [...sources, value];
}

/** 移除一条来源。 */
export function removeSource(sources: string[], value: string): string[] {
  return sources.filter((s) => s !== value);
}
