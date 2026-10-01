# Spec: 流水线"静默降级"收口（截断被当合法、闸门真空通过、梯子不升档）

2026-10-01 grilling 决议（用户逐轮「同意」）。缘起是用户报障「分析 106 看板任务最新一轮
执行过程，看是否有 bug，为什么运行这么久」。

## 事故现场（诊断底稿）

任务 `01M3QW8CKS07R3MWG9XM4FNYER`「修复前端闪屏问题」，2026-09-30T00:43Z 创建，
到 2026-10-01T11:29Z 被手动暂停时已跑 **约 35 小时**、**33 条 run**、**11.4M token**、
30 次调用，仍停在第一阶段 `architect-design`。

最新一轮（2026-10-01T10:23:48Z 用户「取消→重试」之后）：

| run | 节点 | 结果 | 备注 |
| --- | --- | --- | --- |
| 130 | init/execute | success | 90 秒 |
| 131 | architect-design/validate_input | success | 2.4 分钟 |
| 132 | architect-design/execute | **timeout** | 3606 秒（`max_duration_sec=3600`），51 次模型调用 / 89 次工具调用，**全是只读侦察，一个 `write_file` 都没有** |
| 133 | architect-design/execute | cancelled | 自动续接起跑 1.5 分钟后被人工暂停 |

历史回放里同一形态重复出现：architect-design.execute 连续超时 7 次（run 104/106/107/110
+ 被中止的 105/108/109/111），每次 transition 都记「**连续第 0 次**超时」——梯子从没升档；
develop-design / test-design / architect-design 的 validate_output 反复以
`元数据校验失败：missing field \`passed\`` / `missing field \`readiness\`` 失败。

## 三个主缺陷（都有落库数据支撑）

### 缺陷 1：`submit_metadata` 的工具参数被上游截断，管线当"缺字段"处理并静默接受

- 模型**确实提交了** `passed`：run 128 的助手消息正文（`content`）里完整写着
  `<parameter=passed>true</parameter></function></tool_call>`。
- 但落库的 `tool_calls[0].function.arguments` 只有 `{"blockers": [], "feedback": `（29 字符），
  在参数边界处被腰斩；其他样本为 43 / 415 / 1136 字符，**共同点是都停在 `"键": ` 之后**。
- `agent/metadata.rs:95-135` 的 `rescue_truncated_json` 把残片"救"成
  合法前缀（丢弃被截掉的字段），于是下游只能报「缺 `passed`」，而
  `retry_prompt` 又把这个误导信息回灌给模型 → 同样形态再发一次 → 再截断。
- **"看起来成功"的另一半**：必填标量排在切点之前时，节点被判成功而元数据被掏空 ——
  本条链 4 个 execute 节点的 `metadata_json` **全是 `{"readiness":true}`**，
  `test_scenarios` / `acceptance_criteria` / `file_changes` / `affected_files` 全部静默丢失。
- 逃生通道也没兜住：`extract_metadata` 的三级降级（`metadata.rs:55-90`）只认
  ①tool_calls 参数 ②```json 围栏 ③平衡 JSON 对象；而本模型实际产出的
  `<tool_call><function=…><parameter=…>` **XML 文本形态落在三级之外**。
- **截断不在客户端**：流式读路径逐条排查过（`agent/providers/mod.rs:423-570` 的手工 SSE 分帧、
  `openai.rs::parse_chunk`、`ToolAccum` 只 `push_str`、重试不合并部分态、心跳与 `emit_delta`
  出错只走 `Err`、`truncate_messages_json` 只整条丢、`bounded_read` 不在流路径、
  HTTP 客户端只设 connect_timeout 无 read_timeout）——**没有任何地方切片或重置 `arguments`**。
  同 prompt + 转录重放 3 次都返回完整参数，说明触发是间歇的。
- **不是我们的 `max_tokens` 掐的**：`model_request.rs:293` 取 `stage_cfg.max_tokens`，
  architect-design 该列为 NULL → 请求里没带 `max_tokens`；实际完成 token 最高 23842，无上限贴合。
- 客户端的真实缺陷是**静默接受**：`into_tool_call` 只校验 `name`（`providers/mod.rs:802-810`）、
  流优雅 EOF 直接 `break`（`:450-452`）、尾行没有终止换行就不解析（`:468-548`）。

### 缺陷 2：超时梯子被自己产生的中止行清零；重启不给任务级 run 收终态

- 决策 320 的梯子（`pipeline/retry.rs:68-100`，`scheduler/mod.rs:305-460`）：
  连续超时 1–2 次自动续接 → 第 3 次空白重跑 → 第 4 次挂起交人工。
- 实测从未升档：transitions 82/83/85 记的是「连续第 **0** 次」（那时该节点已超时 3/4/7 次）；
  只有 10-01 那次（transition 102）是「连续第 1 次」。
- 原因：`trailing_timeout_streak`（`storage/observability.rs:464`）要求从最新往回**连续**都是
  timeout，撞到第一条非 timeout 即清零；而 `handle_timeout` 判超时时会 `request_cancel`
  掐掉在飞那一轮，那一轮另落一条 `cancelled` 行且 **id 更大**（108 vs 106、109 vs 107），
  于是**自己产生的中止行把刚记上的超时清零**。
- 放大器：2026-09-30 服务被重启 13 次（`journalctl` 全是 `Deactivated successfully`，干净重启）。
  恢复序列（`app/src/serve.rs:575-650`）只处理任务归队、项目级 run、在飞模型请求、值班长半截行；
  `requeue_running_tasks`（`storage/tasks.rs:595`）只翻**任务行**，`abandon_stale_project_runs`
  （`observability.rs:389`）还显式限定 `task_id IS NULL` —— **任务自己的遗留 run 没人收**，
  5 分钟后被 idle 超时判死（duration 记的是 since-start，读起来像"跑了这么久才超时"，实为尸检），
  再制造一批 `cancelled` 行。

### 缺陷 3：validate_output 的 prompt 与 schema 不一致（潜伏）

- `agent/templates.rs:108` 的 `ARCH_VO_SYSTEM` 指示提交 `readiness: boolean`，而
  `ValidateOutputMetadata`（`types.rs:810`）要的是 **`passed: bool`** —— prompt 里的字段在
  schema 里不存在，必填字段反而没提。
- `templates.rs:156`（develop-design）/ `:213`（test-design）的 validate_output 模板
  **一个字段都不列**。
- 本次失败**不是**它导致的（模型写出了 `passed`），故降级为潜伏缺陷，单独修。

## 新发现的缺陷 4：元数据被掏空之后，本该拦它的闸门真空通过

`sync-check` 读的是**落库的 `stage_outputs.metadata_json`**（`executor.rs:1008-1043`）。
现在是 `{"readiness":true}` → `test_scenarios` 数组不存在 → 整段 `design_refs` 引用完整性
校验被跳过（`executor.rs:1044-1088`）→ **直接 `Proceed`**。即：缺陷 1 掏空元数据之后，
唯一能发现"场景清单没了"的关**静默放行**。

## 两个放大器（不是缺陷，但解释了"这么久"）

- **任务描述是空的**：`kanban_tasks.description = ''`，`user-input.md` 只有两个字「同意」。
  architect-design.execute 收到的 prompt 是「标题：修复前端闪屏问题 / 任务描述：（空）」，
  且**只注入 user-input.md 原文**（`model_request.rs:415-444` 的三个重入段里没有 blocker 问答正文）
  —— 下游节点看到"同意"，看不到同意的是**哪两个问题**。模型只能靠翻仓库猜范围，这正是
  "60 分钟、89 次只读调用、零产出"的来源。且 design.md 自述这是前序任务
  `01M3BGVCXDWFPT0Q3BZYAGZP8Q`「连跑四次零产出」后的**重提**。
- **`read_file` 分页失效**：模型把 `offset`/`limit` 写成字符串（`"offset": "1"`），
  `agent/tools.rs:1118` 用 `as_u64()` 读 → `None` → **静默整份读** → 超
  `offload_threshold_tokens=4000` → 卸载；而卸载提示语（`agent/context.rs:295`）又教模型
  「用 read_file 读这个 `.context` 路径」，那文件同样超阈值 → **卸载的卸载**。
  任务目录下堆了 84 个 `.context` 文件 / 1.6MB。同一份 design.md 被连读 7–8 次。

## 决议（逐轮得到「同意」）

1. **交付边界**：走完"诊断 → 修 → 验证"，先立项（本目录），走 `.scratch/` 一票一文件。
2. **缺陷 1 修法**：(a) `extract_metadata` 增一级解析 content 里的 XML 工具调用形态；
   (b) `arguments` 不是合法 JSON 不许记成 `ok`；(c) 救援之后校验必填字段，错误文案写
   "参数被截断"而非"missing field"；(e) 流结束加完整性检查。**(d) `tool_choice` 实验单列（票 07）**。
   → **实验结果（2026-10-01，票 07 `## Answer`）：无效，本票关闭。** 真实 prompt + 转录
   （会话 id 53）各 20 次：三档文本形态都是 0/20（这一组输入没复现现场，触发是间歇的），
   而 `"required"` 这一档 20 次里有 4 次**根本没调工具**（`native=0, finish_reason=stop`）
   ——网关不把这个字段当回事。生产请求体维持不发 `tool_choice`，(a)(b)(c)(e) 是最终答案。
3. **缺陷 2 修法**：两侧都修——(a) `trailing_timeout_streak` 口径排除"由超时中止产生的
   cancelled 行"；(b) 重启恢复给任务级遗留 run 收终态。
4. **缺陷 3**：降级为潜伏缺陷、单独列票；并加一条机械校验（模板提到的字段必须存在于对应结构体）。
5. **空描述**：入口拒绝空描述 **且** 把 validate_input 的"问题 + 推荐答案 + 用户答复"落成
   stage output 并注入到 execute 的 prompt。
6. **`read_file`**：强转数字字符串 **且** 禁止未知/错型参数静默降级；`.context` 产物**免卸载**。
7. **验证**：真实残片固化成单测 fixture（先红→绿），再在 106 上端到端跑一遍。
8. **决策日志**：从 #367 起逐条追加，显式标注修订关系（33/278、320、212、277/79）。
9. **现场处置**：run 行的 `duration_ms`/token **一个字都不改**（台账是历史事实，决策 226）；
   `.context` 留到修复验证后再清（fixture 要从里面取材）；旧 worktree
   `01M3BGVCXDWFPT0Q3BZYAGZP8Q` 是空目录（无 `.git`、无任务目录），可直接删。

### 已撤回的选项（记录在案，防将来重走）

grilling 第二轮曾同意「用 `goto` 从 `architect-design.validate_output` 或 `sync-check`
续走，不重烧 architect-design」。**代码上不成立，已撤回**，证据三条：

- `goto` 只接受目标阶段的**入口节点**，否则 400（`pipeline/resume.rs:285-290`）；
  `sync-check` 被显式拒绝（`resume.rs:278-281`），`sync-check.execute` 在 executor 层也被拒
  （`executor.rs:648-653`）。
- `user_paused` 的 `allowed_actions` 只有 continue / goto（本阶段入口）/ cancel
  （`actions.rs:219-223`）。
- 上游**没有**"有产出就跳过"的机制：executor 只按 `(stage, node)` 派发
  （`executor.rs:424-555`、`637-663`），从不查 `stage_outputs` 是否存在。
  落到任何点都会从那里顺流重跑到底。

唯一能不重跑 architect-design 的办法是**直接改游标行**（把两条设计分支游标摆成
`waiting_join`，只跑纯代码的 sync-check）——那是绕过 `advance` 那扇门，本仓红线（决策 245）。

"回填"能力仍然成立，但用途改为：① 回归 fixture 的真实样本；② 万一续走再撞墙时的备选。

### 106 上那条任务的处置（决议）

先修本批（票 01/02/04）并部署 → 再 **continue**（`resume` 停在 `architect-design.execute`），
顺流重跑 architect execute → validate_output → dev/test 双分支 → sync-check → develop…。
这是唯一能在真实数据上端到端验证修复的受支持路径。

## 批次与阻塞

| 批次 | 票 | 主题 |
| --- | --- | --- |
| 一 | 01 → 04 | 正确性：截断不许静默接受、闸门 fail-closed |
| 一（并行） | 02 | 正确性：梯子与重启恢复 |
| 二 | 03 / 05 / 06 | 契约与输入：模板字段、空描述与注入、read_file |
| 独立 | 07 | 实验：`tool_choice` 强制原生工具调用 |

## 明确不做

- 不改 run 行已落库的 `status` / `duration_ms` / token（历史事实）。
- 不动 `rescue_truncated_json` 对"合法 EOF 截断"的救援能力本身（run59 的先例要保住），
  只改"救援之后怎么判"。
- 不为"从会话重建元数据"加产品入口（一次性脚本，见票 04 的说明）。
- 不调大 `architect-design.max_duration_sec`（先看票 05 的 prompt 改动效果；两个变量一起变
  会让复盘分不清是谁起的作用）。
