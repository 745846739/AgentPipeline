# 01: 一轮里的步骤按实际顺序（段序留痕 + 按段序渲染）

**What to build:** 值班长一轮的留痕此前是三份**聚合**视图（`content` 收口句 / `thinking` 全部推理拼一段 /
`traces_json` 全部工具合成表），**顺序丢了**：界面只能画成「回话在前、思考与回执在后」，而真实的一轮是
「先想 → 查台账 → 再想 → 收口」。本票让后端在那一轮里**按发生顺序**记一份段序落库，界面按它渲染各步，
**收口那一句恒在各步之后**。

**Blocked by:** None（已实现）

**Status:** done（决策 273）

- [x] 新类型 `ForemanSegment`（`{"kind":"thinking"｜"text"｜"tool"}`，工具那一步带 `ok`）+ 迁移 0028 的 `segments_json`；`message_wire` 加加性字段 `segments`
- [x] 收口那一句**不进段序**（它由 `content` 承载）；`text` 段说的是**中途**说出口的话（只在「这次调用还带工具调用」时记）；触到轮数上限那一支弹掉尾部重复段
- [x] 三份聚合列**一个字不改**（各有自己的消费者：折叠块 / 审计表 / 值守轮的 `traces.len()` 门）
- [x] 归约层：`appendForemanEvent` 单出口，`ForemanStreamState.steps` 取代三个桶；末尾若是 `text`，它就是「正在说的那一句」
- [x] 判断层：`TurnStep` 渲染形状；落地轮读段序、**老行由 `thinking` + `traces` 兜底**；三态词合并成一份（`正在查… / 已读 / 未读到`）
- [x] 模板：各步按序画在「过程」那一组里（受控折叠、桌面展开 / 折行档收起、摘要带条数），回话收在最后
- [x] L2：`foreman_persists_the_order_of_the_steps_of_one_turn` / `foreman_without_steps_leaves_the_segments_column_null`
- [x] L3：`the_session_wire_carries_the_ordered_segments_of_a_turn`
- [x] 单测：`foreman.test.ts`（段序 / 相位合并 / 守卫 / 收尾）、`talkTurns.test.ts`（段序 / 兜底 / 末尾正文的两态）
- [x] e2e：`talk.spec.ts` 的「一轮里的步骤按实际顺序」——用 mock 的 `reasoning` 档铺出「想 → 查 → 想」，
      断言 DOM 次序 `['thinking','tool','thinking','reply']`（旧形状下这里是「回话在最前」，会红）
