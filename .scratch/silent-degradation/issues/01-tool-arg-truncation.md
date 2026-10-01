# 01: 工具参数截断不许静默接受（XML 形态解析 + 必填校验 + 救援语义 + 流完整性）

**What to build:** 四处改动，共同把"参数被截断"从**静默降级**变成**要么拿到真值、要么如实报错**。

① **`extract_metadata` 增一级：从 assistant 正文解析 XML 工具调用**。
本模型（`xiaomi/mimo-v2.6-flash`，经 OpenAI 兼容网关）会把完整工具调用写成
`<tool_call><function=NAME><parameter=KEY>VALUE</parameter>…</function></tool_call>` 的**文本**放进
`content`，而结构化的 `tool_calls[].arguments` 被腰斩。现在 `extract_metadata`
（`crates/core/src/agent/metadata.rs:55-90`）的三级降级只认 ①tool_calls 参数 ②```json 围栏
③平衡 JSON 对象——**这个 XML 形态正好落在三级之外，完整答案就摆在手边没人捡**。
新增一级（位置在 ①之后、②之前）：解析 `<parameter=KEY>VALUE</parameter>`，VALUE 先试
`serde_json::from_str`（样本里 `test_scenarios` 的值本身就是 JSON 数组），失败则按裸字符串收。

② **救援之后校验必填字段，错误文案写"参数被截断"**。
`rescue_truncated_json`（`metadata.rs:95-135`）会把残片"救"成合法前缀、丢弃被截掉的字段，
下游于是报 `元数据校验失败：missing field \`passed\`` —— 这个诊断是**误导**，而
`retry_prompt(error)` 还会把它回灌给模型（模型照原样重发 → 再截断 → 连挂 3 次）。
改法（决议 Q4）：救援成功 **且** 必填齐全 → 算成功，但记录里带一条"本参数由截断救援得到"的标记；
救援成功但缺必填 → 失败，文案写「工具参数被截断（缺 `passed`）」，并**在回灌给模型的重试提示里
说明"参数过长被截断，请精简 feedback 或分次提交"**——否则模型只会重发同一坨。
必填字段的判据**不要另写一份**：`types.rs` 的结构体已 derive `JsonSchema`（决策 38 的 tool
parameters 就由此派生），用 `schemars::schema_for!(T)` 的 `required` 读。

③ **`arguments` 不是合法 JSON 不许记成 `ok`**。
`into_tool_call`（`crates/core/src/agent/providers/mod.rs:802-810`）只校验 `name` 存在；
而 `run_stream` 在流优雅 EOF（`:450-452`）与**尾行没有终止换行**（`:468-548`，循环退出后
不 drain 残留 buffer）时都静默收场。补：流结束前必须见过 `[DONE]` 或 `finish_reason`；
残留 buffer 要 drain；**参数完整性不达标时，模型请求的收场状态不许是 `ok`**
（`kanban_model_requests.status`，`agent/recording.rs` 的 `RecordingLlm` 现在对任何 `Ok` 都记 ok）。

④ **`.context` 产物免卸载**（L2 卸载的产物不该再被 L2 卸载，那是无限退套）。
卸载提示语（`crates/core/src/agent/context.rs:295`）现在教模型「用 read_file("{path}") 读完整内容」，
而那个文件同样超 `offload_threshold_tokens`，于是读一次生成一个新的 `.context` 文件——
本次事故里堆了 84 个 / 1.6MB。最小改法：L2 落盘产物（`.context/` 下的路径）在 L2 判据里豁免。

**Blocked by:** None

**Status:** ready-for-agent（2026-10-01；批次一；关键路径）

## 落点

- `crates/core/src/agent/metadata.rs`：`extract_metadata` 增 `MetadataSource::XmlToolCall`
  一级；新增 XML 解析函数；救援后的必填校验与文案。
- `crates/core/src/agent/tools.rs:1181-1191`：`submit_metadata` 现在直接吃
  `rescue_truncated_json` 的结果，要接上"必填是否齐全"的判定。
- `crates/core/src/agent/metadata.rs::retry_prompt`：截断型失败的提示文案要区别于 schema 错。
- `crates/core/src/agent/providers/mod.rs`：流结束完整性检查 + 残留 buffer drain；
  `ToolAccum::into_tool_call` 的 JSON 完整性判据。
- `crates/core/src/agent/recording.rs`：收场状态与"参数完整性"挂钩。
- `crates/core/src/agent/context.rs`：L2 豁免 `.context/` 路径。

## 验收

**真实残片必须固化进单测**（这是本票最重要的产出——样本是被真实网关注出来的，构造不出来）。
本次事故的实测 `arguments` 前缀，四段，长度 29 / 43 / 415 / 1136 字符：

- `{"blockers": [], "feedback": `（conv 99 / conv 92，validate_output 缺 `passed`）
- `{"readiness": true, "test_scenarios_path": `（conv 88，**被判成功但 `test_scenarios` 静默丢失**）
- `{"blockers": ["场景 3（high）假空态文案引错：…"], "feedback": `（conv 93）
- `{"file_changes": [{"action": "create", "path": "frontend/e2e/frameProbe.ts"}, …], "dev_doc_path": `（conv 89，缺 `readiness`）

配套的 **XML 正文**样本同样固化（conv 98 那段 2517 字符、八个字段齐全的
`<tool_call>`；conv 88 那段 6567 字符、含完整 `test_scenarios` 的）。

- [x] 四段残片：新解析器能从**同一条消息**的 `content` XML 还原出完整对象，八/三个字段一个不缺
- [x] 四段残片若**没有** content XML 可回退 → 失败，且错误文案含「被截断」不含「missing field」
- [x] `conv 88` 形态：救援成功且必填齐全 → 判成功，但记录带截断救援标记（不许静默）
- [x] 回归：`rescue_truncated_json` 对 run59 那类"合法 EOF 截断"的能力**不受损**（既有测试全绿）
- [x] 流完整性：造一个缺 `[DONE]` 的流与一个尾行无换行的流，两者都必须被识别（不许静默 ok）
- [x] `.context` 再读不再产生新的 `.context` 文件
- [x] `make test` / `make check` 全绿（本地按决策 331：`make check-lint`（fmt + clippy -D warnings）与 `cargo test -p agentpipeline-core` 的 lib / integration 两层全绿；`make check` 的全量（含前端与 e2e）留给 CI）

**落地记录（2026-10-01）**

- `metadata.rs`：`MetadataSource::XmlToolCall` / `find_xml_tool_call` / `TRUNCATED_ARGUMENTS_MARKER`
  / `validation_failure_error` / `any_tool_arguments_broken` / `broken_arguments_note`；
  `retry_prompt` 对截断型失败换措辞。单测 23 条（含真实残片与 XML 正文 fixture）。
- `model_invoke.rs`：类型化校验失败 → 先试 XML 正文还原；不成且参数是坏的 → 按「截断」报错，
  原始 schema 诊断只进日志（**不进**回灌文案）。
- `tools.rs`：`submit_metadata` 走救援时留一行 `warn`（任务/stage/node/run/原串长度）；
  `apply_l2_offload` 增 `reads_offload_artifact` 豁免（票 01④）。
- `providers`：`StreamChunk::FinishReason` 两个适配器都发；驱动层 drain 残留尾行；
  `[DONE]`/`finish_reason` 都没见过的 EOF → `Err`；`AgentResponse.finish_reason` 落库。
- `recording.rs`：参数不是合法 JSON 时收场记 `error`（不再 `ok`），说明带上游收尾原因。
- `testkit`：新增 `Step::ToolRaw` / `submit_metadata_raw`（表达「上游腰斩参数」的形状）。


**明确不做**：不重写 `rescue_truncated_json` 的救援算法；不为"从历史会话重建元数据"加产品入口
（一次性脚本，见票 04）。

**来源：** `.scratch/silent-degradation/spec.md` 缺陷 1；落库证据
`kanban_node_conversations` id 99/88/89/92/93/98（`arguments` 残片 vs `content` 完整 XML）。
