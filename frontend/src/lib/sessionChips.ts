import type { ForemanSessionMeta } from '../api/types';

/**
 * 班次 chip 行的取舍（票 06，纯函数）：「显示已归档」开关显不显示归档。
 *
 * 缺省（`showArchived = false`）：归档的不列——「从列表里收起来」照旧是归档的
 * 全部含义（决策 204⑦ 的口径没动，动的只是列表给谁看）；开启则照列，灰不灰由
 * `archived_at` 在不在判（那是渲染层的牙齿，不在这条函数里）。
 *
 * **关掉开关就是回到现状，行里一个归档都不留——哪怕正读着的那一班也是**：
 * 「显示已归档」关着却仍挂着一枚归档 chip，这开关就是在说谎（票面「关开关复原」）。
 * 「正在读归档班」这件事另有两处兜：`reload()` 的 `known` 多认已在读的那一班
 * （不把人弹去默认班），以及身份行（⋯ 菜单）直接取 `session.session`、不走这条
 * 过滤——「选中项不弄脏」靠的是列出来时灰而选中，不是让它永远在场。
 *
 * 归并判据在**这里**不在组件里：chip 行与 ⋯ 菜单共用它，两处各写各的迟早分叉。
 */
export function chipRow(
  sessions: readonly ForemanSessionMeta[],
  showArchived: boolean,
): ForemanSessionMeta[] {
  return sessions.filter((s) => showArchived || !s.archived_at);
}
