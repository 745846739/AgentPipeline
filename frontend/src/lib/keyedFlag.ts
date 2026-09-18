/**
 * 按 key 键控的「在飞」集合（票 12 / R2-17）。
 *
 * 为什么要有它：市场页有三处同形的状态——正在读的仓、正在刷新的仓、正在安装的技能目录
 * （`dir` 在仓内唯一，比名字可靠）。它们都要「同一个 key 并发时，一次完成不许清掉另一次
 * 的在飞态」，也就是「置位 / 清位只动自己那一格」。此前这条形状在各处手抄了五遍
 * （`{ ...x, [k]: true }` 与 `{ ...x }; delete next[k]`），抄漏一处就退回单槽的旧毛病。
 *
 * 纯函数、返回新对象：Svelte 的 `$state` 靠赋值触发更新，所以调用点一律
 * `flags = setFlag(flags, key, true)`。
 */
export function setFlag(
  flags: Readonly<Record<string, true>>,
  key: string,
  on: boolean,
): Record<string, true> {
  if (on) return { ...flags, [key]: true };
  // 只删自己那一格：别人的在飞状态不是这一次的结果
  const next = { ...flags };
  delete next[key];
  return next;
}

/** 该 key 此刻是否在飞。 */
export function isFlagged(flags: Readonly<Record<string, true>>, key: string | null): boolean {
  return key !== null && flags[key] === true;
}
