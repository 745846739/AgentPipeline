# 02: 接上路径与发送中不可区分——点亮 streaming + 半截行拼接

**What to build:** 刷新 / 切页返回 / 本地放弃后接上的一轮，形态与「本机在发」一致：
`syncFollowing` 接手时点亮 `streaming`（光标、「正在想」ticker、贴底跟随全部恢复）；
快照里的决策 312 `in_flight` 半截行与 `seq > seq0` 尾巴**拼成一条 live 轮**渲染，
不再「半截行落地式 + 尾巴单独成轮」；POST 在途时返回不再乐观轮与台账行说两遍。

**Blocked by:** None

**Status:** done（已实现，决策 314）

- [x] `syncFollowing` 立锚那两支（刷新接上 / 落地后又起一轮）把 `stream.streaming` 置真；
      收口路径（settled / lost）照旧熄灭
- [x] `lib/talkTurns.ts`：快照含 `status='in_flight'` 半截行且 `turn_in_flight` 为真时，
      半截行 + 尾巴步骤合一条 live 轮（thinking / 工具步骤 / 正文与发送中同构渲染）
- [x] POST 在途返回：快照里已有 user 行时乐观轮退场（`pendingText` 清掉），不再双句
- [ ] 单测：接上路径的轮渲染形态与发送中逐字段一致（streaming / 光标 / live 标记）
- [ ] 单测：半截行 + 尾巴拼接后无重复步骤、seq 去重判据不回退（票 02 of talk-replay）
- [ ] 实测：发送中切去看板再回对讲台，形态与未切走时不可区分

## Comments

- 2026-09-29 实施完毕（决策 314）。前三项勾掉，单测落在 `talkTurns.test.ts`（拼接组重写 +
  新增工具尾巴 / 段序前缀 / 去重四支）与 `talk.test.ts`（点亮三支）；实测条待验。
