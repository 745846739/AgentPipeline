# 02: 压缩硬底 token 化（conversation_max_tokens=300k）+「管线压缩」设置卡

**What to build:** 从用户视角：L3 压缩的触发线从「20 万字符」换成「30 万 token」——
审计级任务（单次请求最大 8.9 万 token）不再触发任何压缩，上下文连续性拉满；同时
设置界面多一张「管线压缩」卡，硬底线与压缩保留轮数在界面上直接可调、保存即对下一轮
生效，不用改 config.toml 重启。

**为什么**：ux-audit-3 触发了 **51 次** char_floor 压缩（develop 22 / test 15 / review 12 /
设计 2），每次压缩作废提示词缓存前缀并压掉一部分工作上下文。20 万**字符** ≈ 约 5–10 万
token，对 100 万 token 的模型窗口保守过头。决策 376 票 03 当初选字符硬底，是因为
token 侧的**容量判据**被 provider 窗口登记架空（登记虚高 → 软限永不触发）；但字符硬底
因而背上了「对窗口大的模型压得过早」的副作用。token 硬底是**绝对数、不看 provider 登记
的脸色**（不重蹈「登记虚高架空判据」的覆辙），估算器复用软限同款
（`estimate_messages_tokens`，误差有界、`context.rs` 注明允许 1.5 倍松弛），触发点随
估算误差浮动 ±五成也在 100 万窗口的安全区内。**显式推翻**决策 376 票 03 的字符硬底
判据与决策 377 后续「不改 `conversation_max_chars` 缺省」的口径（新决策在案说明）。

**形状**：

- **触发判据**（两端同源，决策 291——流水线与值班长吃同一份，同批改）：
  `should_compact_with_floor` 签名改为 `(current_tokens, capacity, conversation_max_tokens)`，
  判据 = 软限（token 估算 vs 容量，不变）**或** `current_tokens > conversation_max_tokens`；
  `transcript_chars` 参数从判据中删除（字符读数只保留在观测日志里）。`capacity = None`
  时软限不判、token 硬底照判（与票 03 的姿态一致：本地算术不臆造窗口）。
- **配置字段**：`Settings` / `PipelineOverrides` / `set!` 宏新增
  `conversation_max_tokens: usize`，缺省 **300_000**；`conversation_max_chars`
  **保留缺省 200_000 不动**，职责收缩为「会话落库截断」（0017 三段共账 + 0038 reasoning
  截断，`executor.rs:383` 注入路径与 `observability.rs` 读取点一字不动）。
- **运行时可改（DB 覆盖层，照 server_bind / offload 样板）**：应用从不写 config.toml
  （settings-honesty 定下的边界），故 UI 保存走**新迁移的单行覆盖表**
  （`CHECK (id = 1)`，两列均可空：NULL = 没保存过回落 config 值，非 NULL = 覆盖）；
  Store 读写方法带「None=读配置」语义注释。
- **生效点（懒读，照 foreman_watch 样板）**：流水线侧在 attempt 组装前（executor 的
  `ModelInvoke` 构造处，store 在手）读覆盖、就地覆盖 settings 克隆的两个字段；
  值班长侧在 TurnFacts 组装处同样懒读——两侧同批，决策 291 的同源约束不断。
  子代理路径（`SubAgentRunnerConfig`）随 `ModelInvoke.settings` 自动吃到。
- **API**：新路由模块 `routes/compaction.rs`，`GET /compaction` / `PUT /compaction`
  （照 `/foreman-watch` / `/offload` 形状：GET/PUT 同一份 readout，PUT 保存后回重读读数；
  provenance 逐字段 `default` / `settings`——保存值等于缺省也是 `settings`，与
  foreman_watch 口径一致）。PUT 校验：两字段必须 > 0；缺体 422。
- **前端**：新设置卡「管线压缩」（独立路由，照 SettingsForeman 族：`$state` 四件 +
  load() 重读为权威 + 保存后以重读读数反馈；路由类型 / parseRoute / App.svelte /
  ROUTE_TITLES / SettingsLanding「怎么跑」分类五处注册）；暴露两个字段：
  `conversation_max_tokens`（硬底线）与 `keep_recent_rounds`（压缩保留轮数，决策 376
  时已是配置项但一直没进 UI）；`conversation_max_chars` 不进 UI（帮助文案带一句它管
  落库截断、默认 20 万字符）。
- **观测**：压缩日志的 `trigger` 取值 `char_floor` → `token_floor`（软限触发的
  `soft_limit` 不变），`floor` 字段记 token 数，`chars` 保留供对照。

**明确不做**：不动 `estimate_messages_tokens` 的算术与 L1/L2 裁剪口径；不动软限/硬限
比例（`context_soft_limit_ratio` / `context_hard_limit_ratio`）；不拆「压缩触发线」与
「落库截断」为一本账之外的第三本账（token 线管触发、字符线管落库，单位不同必然分家，
各管各的并在决策里写明）；不做「设置卡改 `conversation_max_chars`」。

**Blocked by:** None

**Status:** done

- [x] 单测：新谓词——token 超硬底触发、不超不触发；`capacity = None` 时软限跳过、
  硬底照判（沿用票 03 的教义用例形状，改 token 口径）
- [x] 单测：配置清单同一性定值探针自动覆盖新字段（`every_setting_field_is_actually_overridable`
  不改锚即过）+ `defaults_match_design_table` 补 `conversation_max_tokens: 300_000`
- [x] 集成测试：`a_long_tool_round_trip_node_is_capped_by_the_char_floor` 重写为
  token 口径（注入小 `conversation_max_tokens`，60 轮工具往返节点照常收口、压缩真实
  发生、trigger=token_floor、请求总量低于 O(n²) 理想值的一半）
- [x] 集成测试：落库截断回归——`conversation_max_chars` 仍按字符截断三段共账
  （既有用例改注入口径，语义不变）
- [x] API 契约：GET 缺省 `origin: "default"`（值等于 config 值）、PUT 后重读
  `origin: "settings"`、缺体 422、非法值 400
- [x] DB 层：迁移跑通（新表 CHECK(id=1)）、NULL 列回落 config 值、保存/重读往返
- [x] 前端：组件测试照 SettingsForeman.test.ts 姿态（可访问性契约 + 重读权威）；
  SettingsLanding 门牌与新路由注册被 e2e 资产守卫覆盖
- [x] 决策日志续写一条（显式推翻决策 376 票 03 字符硬底判据 + 记账「token 线管触发、
  字符线管落库」的分家；`docs/overview.md` 配置表补 `conversation_max_tokens` 行）
