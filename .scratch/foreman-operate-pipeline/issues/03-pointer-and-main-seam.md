# 03: 点名注入 + 主缝三件断言

**What to build:** 端到端打通「人下令、值班长按手册提议、按键执行」：foreman 阶段配置行的 persona_append 默认播种一句点名（遇操作类指令先按名拉 operate-pipeline 手册），每一轮——人对话轮与值守轮——的 system prompt 都带着它；值班经理在对讲台里说「把 X 推进一步」，值班长拉取手册正文、按手册翻译成 task 提议，提议参数与按钮直发的参数逐字一致，按键即执行落库。用户在设置里改过或关掉点名则尊重用户值。对讲台 UI 零改动。

**Blocked by:** 02 (出厂技能种入与不可删)

**Status:** done（已实现；端到端 demo 并入票 04 黄金剧本 A 类执行）

- [x] 主缝断言①：人对话轮模型真正收到的 system prompt 含点名句；值守轮同样钉一条
- [x] 主缝断言②：脚本驱动模型发 Skill(name=operate-pipeline)，工具回执是手册正文（工具层真实执行）
- [x] 主缝断言③：拉完手册后产出的 task 提议 args 与配对端点按钮直发的参数逐字一致
- [x] persona_append 默认播种只写一次默认值：用户改过/清空过不被启动逻辑覆盖
- [x] persona_append 的写入与回读经阶段配置端点契约测试钉住
- [x] 无该配置存量数据（升级路径）补播种（L2 `pointer_seeds_a_missing_foreman_row` + L3「删行 → 重播补回」）
- [ ] 端到端 demo：对讲台下令 → 提议卡出现 → 按键执行成功（**随票 04 黄金剧本 A 类执行**：同一句话的验收，不重复立条；按键落库的机器证据已由 L3 `pressing_a_task_resume_proposal_lands_the_cursor` 钉住）
- [x] 既有 e2e 断言（回话里没有按钮等）继续全绿——UI 形状未变
