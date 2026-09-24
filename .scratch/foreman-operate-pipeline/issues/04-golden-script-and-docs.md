# 04: 黄金剧本人工验收 + 文档同步

**What to build:** 收口验收：用真 LLM 按票 01 起草的黄金剧本在对讲台走一遍（opt-in 姿态、不进默认门，照真 LLM 冒烟先例），逐条记录期望 vs 实际——单票推进、批量重试（逐票一卡、跨任务排队）、只读问答（引用 pending 原因原文、不产提议）三类都要过；手册措辞若教出了蠢行为，回改手册直至剧本全过。同时补齐文档：术语表新增「出厂技能」词条（白名单 / 幂等种入 / 不可删三要素），testing.md 决策↔测试映射表补行。

**Blocked by:** 03 (点名注入 + 主缝三件断言)

**Status:** done（2026-09-24 黄金剧本三类场景真 LLM 全过；一次断言红已按「先改手册再重跑」回改——见 Comments 执行记录）

- [x] 黄金剧本三类场景全部人工执行通过，记录贴回本票 Comments（2026-09-24，真 LLM 跑通，三段记录见 Comments）
- [x] 单票推进：一句自然语言 → 参数正确的提议卡 → 按键执行落库（A 类过：goto 提议带全落点；「按键落库」半步的机器证据 = L3 `pressing_a_task_resume_proposal_lands_the_cursor`，参数同源 = 主缝断言③）
- [x] 批量重试：一次下令 → 每任务至多一张在途卡、排队给人逐张按；同任务连锁被教成分步提（B 类过：t2/t3/t4 各恰一张 retry，回话明说「一票一按」）
- [x] 只读问答：回答引用 pending 原因原文、全程零提议（C 类过：停因「冲突了两条路，你挑一条」逐字引用，零提议）
- [x] 手册按剧本暴露的问题修订完毕（剧本第一轮 goto 缺落点断言红 → 手册 `resume` 条目补「target 抄成 target_stage/target_node」→ 重跑全绿）
- [x] 术语表含「出厂技能」词条且三要素齐
- [x] testing.md 决策↔测试映射表补行，锚点指向票 02/03 的真实用例
- [x] 决策日志、术语表、testing.md 之间无悬空引用（编号与词条名互相对得上）

## Comments

### 2026-09-24：文档同步完成；黄金剧本装置就位、执行被本机 LLM 代理挡住

**已完成（上面打勾的三项）**：术语表新增「出厂技能（factory skill）」词条（白名单 / 幂等种入 /
不可删三要素齐，附点名指针的归属说明），并在「技能」条的 172① 表述上加了 261 的窄口修订标注；
`docs/testing.md` 决策↔测试映射表补了 261 行，锚点指向票 02/03 的**真实用例名**（`factory.rs` 9 条、
`foreman.rs` 主缝 4 条、`api_contract.rs` 2 条、`serve.rs` 源码扫描守卫）；三份文档互引核对
（决策 261 ↔ 词条名「出厂技能」↔ 测试锚点 ↔ AGENTS.md 计数 #1–261）无悬空。

**黄金剧本的执行装置**（照真 LLM 冒烟的 opt-in 姿态，不进默认门）：
`crates/core/tests/integration/foreman_golden.rs::golden_script_three_scenarios_with_a_real_llm`
——`#[ignore]` + 环境变量双锁（`AGENTPIPELINE_SMOKE_*`，与 `llm_smoke.rs` 同一副），
fixture 在临时 home 里摆出 A/B/C 三类场景（t1 pending 可推进 / t2–t4 终态 / t5 停因原文），
播种走**与生产同一个** `seed_factory_defaults`，三句剧本指令各开一班真对话，逐场景打印
回话与提议，并结构断言：A 恰一张 `resume` 提议且 `resume_action` 来自此刻 `allowed_actions`
（goto 带落点、按键前任务不动）、B 逐票三张且每任务至多一张在途、C 零提议且**停因原文逐字**
在回话里；写轮（A/B）都断言真拉了手册（traces 含成功的 `Skill` 调用）。
**skip 路径已验证**（未设 key → eprintln 跳过、测试绿）。

**执行记录（阻塞）**：本机唯一配置的 provider 指向 `http://127.0.0.1:8787/v1`（家目录
`data/agentpipeline.db` 的 providers 表，openai 协议 / `xiaomi/mimo-v2.6-flash`），执行时该端口
**没有监听**（`curl` 返回 000、`lsof` 无 8787）——A/B/C 三类一次都没有真跑。代理起来后一条命令：

```bash
AGENTPIPELINE_SMOKE_VENDOR=openai \
AGENTPIPELINE_SMOKE_MODEL=xiaomi/mimo-v2.6-flash \
AGENTPIPELINE_SMOKE_API_KEY=<家目录 providers 表里的那把 key> \
AGENTPIPELINE_SMOKE_BASE_URL=http://127.0.0.1:8787/v1 \
cargo test -p agentpipeline-core --test integration golden_script -- --ignored --nocapture
```

跑完把打印的三段记录贴回本节；**结构断言红 = 手册教得不对，先改手册（票 01 的正文文件
`crates/core/src/agent/factory/operate-pipeline/SKILL.md`）再重跑**，措辞质量人工确认。
票 03 留的「端到端 demo」同理：提案卡出现与按键落库在真对话里顺手各截一次即可。

**还差的一半在哪**：上面未打勾的 5 条全部是这一次被挡住的执行项——文档与装置都已就位，
缺的只是 8787 那个代理开口。

### 2026-09-24（code-review 收口）：两轴复查后的修口

**Standards**：① `serve.rs` 的源码扫描守卫原先是**自证式**的（要找的字面量就躺在被扫描的
测试段里，删掉生产调用也绿）——改为切掉 `#[cfg(test)]` 段只扫生产段；② 卸载报文里的
空格 run 与指错的开关（「不要声明它」→ 实际在用的开关是 foreman 行的 `persona_append` 点名）
一并改写；③ glossary「技能」条随之失真的两句绝对断言（「两条路」→ 三条加出厂种入、
「一律由用户安装」→ 安装或种入）；④ `skills.rs` 模块头我上一版写出的缺连接词句改齐。

**Spec**：⑤ **真 bug——设置页清空点名会被启动播种顶回**：`buildStageConfigPut` 清空时
省略字段 → 后端存 NULL → 下次启动当「没配过」重播，决策 6 的「可关」关不住。修为
**foreman 行清空时显式下发 `""`**（其余阶段的「留空 = 省略 = 清空」一个字不动），
vitest 补两侧各一条；L3 契约本来就钉了「`""` 不被重播覆盖」，前后自此接上。
⑥ 断言③只钉了「参数同源」、没钉「按下去落地」——补 L3
`pressing_a_task_resume_proposal_lands_the_cursor`（`run_task_tool::resume` 分支的按键
快乐路径此前零覆盖：既有按键用例只有环境两族 / `service` / 拒执那条）。⑦ 票 03 的
捆绑勾拆开（升级路径已实现打勾，demo 留给剧本）。

**接受不改（记录在案）**：`foreman_golden.rs` 的 opt-in 装置与 Out-of Scope「黄金剧本的
自动化」不冲突——它不进默认门（`#[ignore]` + 环境变量双锁），跑它仍是人工验收动作；
`park` 助手在 `foreman.rs` / `foreman_golden.rs` 各留一份（同 crate 私有 helper，已注释
说明取样理由）；L3 URL 里的字面技能名（路径段必须字面，内嵌正文比对仍走 `FACTORY_SKILLS`）。
黄金剧本 5 条执行项仍被 8787 代理挡着（见上节）。

### 2026-09-24（黄金剧本执行）：三类场景真 LLM 全过；一轮断言红已回改手册

**链路**：本机代理 `npx commandcode-api-proxy` 起在 `127.0.0.1:8787`（v0.4.1，auth 启用），
上游 commandcode.ai；首跑撞余额不足（`insufficient credits`，换 key）后放行。运行命令即上节
那条（`AGENTPIPELINE_SMOKE_*` 四件套 + `golden_script -- --ignored --nocapture`）。

**第一轮断言红 → 改手册**：A 类模型提了 `resume_action=goto` 但**没带落点**——数据其实都在
（`read_task` 的 `allowed_actions` 里 goto 条目带 `target:{stage,node}`，工具 schema 也声明了
`target_stage`/`target_node`），漏在手册：`resume` 条目只教了 `task_id`+`resume_action`，
没教「goto 必须把 target 抄成 target_stage/target_node（与确认按钮同源，缺了按键必 400）」。
按本票协议先改手册 `crates/core/src/agent/factory/operate-pipeline/SKILL.md` 再重跑。

**第二轮全绿**（77.8s，token A 24998/303 · B 25931/644 · C 11850/590）：

- **A 单票推进**（指令「把 t1 推进一步」）：痕迹 `Skill → read_task → task`（真拉手册）；
  恰一张提议 `{"action":"resume","resume_action":"goto","task_id":"t1","target_stage":"init","target_node":"execute",...}`——
  落点齐、`resume_action` 来自此刻 `allowed_actions`、按键前任务仍 Pending。回话附停因原文
  「重试耗尽，等你拍板」并明说「未执行，等你按键确认」。
- **B 批量重试**（指令「这三张挨个重试」）：痕迹 `Skill → read_task×3 → task×3`；
  t2/t3/t4 各恰一张 `retry`、零多余卡，回话摆出顺序表 +「一票一按，按顺序来」。
- **C 只读问答**（指令「t5 为什么停了」）：痕迹 `Skill → read_task`；**零提议**；停因
  「冲突了两条路，你挑一条」逐字引用，还注明 `user_decision` 枚举与「等你按键的决策点，
  不是故障」。

**改手册后的回归**：factory 10 条 + foreman 11 条（含主缝 4 条）+ app 契约 4 条
（factory_skill / pointer / foreman_pointer / pressing）+ core lib 509 条全绿，clippy/fmt 过。

**措辞质量**：三段回话贴在上面，措辞人工复核——记录在案待值班经理过目（结构断言已机器钉死）。
