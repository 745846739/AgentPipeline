import type { ChatMessage } from '../api/types';

/**
 * 选中 run 的消息过滤（spec list-windowing：会话页签只做「消息内容 / 角色过滤」）。
 *
 * 一个关键词同时对**消息内容**与**角色名**（user / assistant / system / tool）做
 * 大小写不敏感子串匹配——「把 tool 行都翻出来」与「含某个词的那几句」是同一个框。
 * 判据住 lib 不住组件（与 `commandFilter` / `runFilter` 同一形状）：过滤是规格，
 * 组件只管接线。
 *
 * 返回谓词而不是列表：组件里消息带着**原数组下标**当渲染键（窗口化后局部 i 会错位），
 * 先 map 出下标再过滤，键才不随窗口滑动漂移——所以过滤发生在下标之后、切片之前
 * （先过滤后切，决策 319 的口径）。
 */
export function messageMatches(
  message: Pick<ChatMessage, 'role' | 'content'>,
  query: string,
): boolean {
  const q = query.trim().toLowerCase();
  if (q === '') return true;
  return `${message.role} ${message.content ?? ''}`.toLowerCase().includes(q);
}
