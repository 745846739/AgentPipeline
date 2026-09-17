# 02: 未知工具名 fail fast

**What to build:** 把「阶段配置声明了 v1 不存在的工具」从**静默丢弃 + warn** 改为 **fail fast**，
与 `deny_unknown_fields`（决策 47 / 103 / 134 姿态）和「引用不存在的 skill → 拒绝启动」
（`crates/core/src/config.rs::validate_startup`）对齐。触发场景：前端 `StageConfigForm.svelte` 的
`tools_json` 是自由文本输入，用户可写入任意工具名，而当前唯一后果是一行 `tracing::warn!`。

**Blocked by:** None (can start immediately)

**Status:** done（2026-09-17）

**背景：** `executor.rs:3072-3075` 对任何不在 `BUILTIN_TOOLS` 中的名字打 warn 后 `continue`。
决策 154 收口后，`spawn_sub_agent` 不再是一个特例——本票处理的是通用姿态。姿态理由与
`docs/agents.md` 的升级注意同源：「静默忽略会让『配置写了却没生效』无从察觉」。

- [x] 声明未知工具名时**拒绝**：写入阶段配置（`PUT /stage-configs/{stage}`）即报错，
      错误信息列出未知名字与 v1 已知工具集（**9 个**：8 内置 + `spawn_sub_agent`，见下）
- [x] 校验与「引用不存在的 skill」同层同源（复用 `validate_startup` 的姿态与报错风格），
      不做第二套校验实现
- [x] `tool_defs` 中的 warn + 丢弃分支在启动路径上变为不可达（保留或删除需在实现时定，
      但须保证不会有两条路径对同一错误给出不同处置）
- [x] 存量配置的升级路径明确：库中已有含未知工具名的 stage_config 时，启动行为必须**确定**
      （fail fast 报错并指明是哪个 stage 的哪个名字），不静默放行——若选择自动清理须写明理由
- [x] `docs/agents.md` 补一条升级注意（与 `deny_unknown_fields` 那条并列），说明
      「拼错的工具名由静默忽略改为启动/写入报错」是行为变化
- [x] `docs/testing.md` §11 或相应契约节记录该收紧；L1 / L3 各有一条用例钉住（拒绝写入 +
      启动时存量数据的确定性处置）
- [x] 前端 `StageConfigForm` 在收到 400 时呈现后端错误信息（不吞错、不伪造成功）
      ——**实测**：链路早已存在（`SettingsStages.svelte` 的 `catch` → `StageConfigForm` 的
      `error` 槽 → `.error` 块），本次补两条用例把它钉住（客户端回显 + 表单渲染），并修掉
      一处**自相矛盾**：`tools_json` 的占位符原本示例 `{"execute": [...]}` 的对象形态，
      而后端从来没有过那个形态（旧行为是静默忽略，现在是 400）——占位符改为数组形态。
- [x] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 vitest / svelte-check / build

## 交付（2026-09-17 实现）

**判据只有一处：** 新增 `crates/core/src/agent/client.rs::is_known_tool_name` / `known_tool_names`
= `BUILTIN_TOOLS`（8）+ `SPAWN_SUB_AGENT_TOOL`。工具定义的生成（`executor::tool_defs`）与配置准入
（`config::validate_startup`，`PUT /stage-configs` 复用同一函数）问的是同一个谓词——两处各写一份的
后果是「写入时说不知道这个名字、运行时却把它丢掉」这种只能靠现象定位的漂移（与
`tools::denied_by_tier` 同一姿态）。

**报文只有一句（代码审查后收口）：** `client::unknown_tools_message` 是这句报文的**唯一**产地，
`config.rs` 与 `executor.rs` 都调它。审查前两处各拼一句（一处列全部未知名字、一处只列一个），
「同一个错误一种处置」这句话就只兑现了一半——同一种处置也要同一种说法。

**三处落点：**

1. **写入 / 启动校验**（`config.rs`）：新增 `parse_tool_names`（形态检查——非数组、非字符串项
   一律 [`Error::Config`]）与 `validate_startup` 里的一段校验。报文同时给出**未知名字**与
   **v1 已知工具集**（照它改配置即可）。存量配置走同一条路：启动即失败并指明阶段 + 名字，
   **不静默放行、也不自动清理**——自动清理会把用户的错字悄悄抹掉，让人再也看不到自己写错了什么。
2. **执行侧**（`executor.rs::tool_defs`）：warn + 丢弃分支改为**返回 `Err`**（`tool_defs` 改为
   `Result<Vec<ToolDef>>`，多收 `stage` / `node` 两个枚举——只进报错信息，故不预拼字符串，
   免得为一条不可达分支每轮多分配一个 `String`）。它在启动路径上不可达，留着是为了
   **不给同一个错误第二种处置**：手工改库绕过校验时，行为与写入时一致——拒绝。
3. **前端**：无需新代码——`SettingsStages.svelte` 的 `submit` 早已把 400 的 `ApiError.message`
   交给 `StageConfigForm` 的 `error` 槽（`shownError` → `.error` 块）。本次补一条客户端用例
   （`frontend/src/api/client.test.ts`）钉住新报文的回显，免得日后有人把这句吞掉。

**票面的一处陈旧（就地更正）：** 票面第 1 条写「v1 已知工具集（7 个内置）」，那是 2026-09-13 的
数目。**决策 172③ 之后是 8 个内置**（新增 `Skill`），加上扩展工具 `spawn_sub_agent` 共 **9 个**。
实现按 9 个走，报文里列的就是这 9 个。另：票面提到的 `StageConfigForm.svelte` 现已位于
`frontend/src/components/settings/`（页面是 `frontend/src/routes/SettingsStages.svelte`），行号
（`executor.rs:3072-3075`）也已随代码演进移到 `tool_defs` 内。

**用例：**

- L1 `crates/core/src/config.rs::unknown_tool_names_fail_startup_validation`（9 个已知名逐个通过 +
  未知名拒绝 + 报文含名字与已知集合 + 非数组 / 非字符串项拒绝）；
- L1 `crates/core/src/agent/client.rs::known_tool_names_is_builtins_plus_extended`（判据不许比
  内置集多认一个名字）；
- L1 `crates/core/src/pipeline/executor.rs::tool_defs_rejects_unknown_names_and_accepts_the_known_set`
  （同一种处置，报文含阶段 + 节点）；
- L3 `crates/app/tests/api_contract.rs::stage_config_write_rejects_unknown_tool_names`
  （400 + 报文形状 + 不落库 + **9 个已知名字逐个可写**）；
- L3 `…::startup_rejects_an_existing_stage_config_with_an_unknown_tool`（存量数据的确定性处置）；
- 前端 `frontend/src/api/client.test.ts`（400 报文原样回显）+ `frontend/src/components/settings/StageConfigForm.test.ts`
  （表单原样渲染那句报文；提交不私自改建名）。

**一处超出票面的收紧（显式记录）：** 票面只要求「未知**名字**拒绝」，实现连**形态**一起收了
（非数组 / 非字符串项）。理由：那正是同一类「写了却没生效」的静默路径（旧 `json_string_list`
会把它们滤掉），而票面的姿态就是关掉这类路径。**代价说清**：一个此前能被保存、但后端从来
没实现过的对象形态（`{"execute": [...]}`）现在会 400——UI 占位符同步改掉，不再示例它。

**文档：** `docs/agents.md` §10.6 升级注意（与 `deny_unknown_fields` 那条并列）；
`docs/testing.md` §5 新增「工具集准入」行、§7 新增 `stage-configs` 行、§11 新增收紧记录段。
