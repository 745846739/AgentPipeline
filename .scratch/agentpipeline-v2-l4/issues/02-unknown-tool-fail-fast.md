# 02: 未知工具名 fail fast

**What to build:** 把「阶段配置声明了 v1 不存在的工具」从**静默丢弃 + warn** 改为 **fail fast**，
与 `deny_unknown_fields`（决策 47 / 103 / 134 姿态）和「引用不存在的 skill → 拒绝启动」
（`crates/core/src/config.rs::validate_startup`）对齐。触发场景：前端 `StageConfigForm.svelte` 的
`tools_json` 是自由文本输入，用户可写入任意工具名，而当前唯一后果是一行 `tracing::warn!`。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**背景：** `executor.rs:3072-3075` 对任何不在 `BUILTIN_TOOLS` 中的名字打 warn 后 `continue`。
决策 154 收口后，`spawn_sub_agent` 不再是一个特例——本票处理的是通用姿态。姿态理由与
`docs/agents.md` 的升级注意同源：「静默忽略会让『配置写了却没生效』无从察觉」。

- [ ] 声明未知工具名时**拒绝**：写入阶段配置（`PUT /stage-configs/{stage}`）即报错，
      错误信息列出未知名字与 v1 已知工具集（7 个内置）
- [ ] 校验与「引用不存在的 skill」同层同源（复用 `validate_startup` 的姿态与报错风格），
      不做第二套校验实现
- [ ] `tool_defs` 中的 warn + 丢弃分支在启动路径上变为不可达（保留或删除需在实现时定，
      但须保证不会有两条路径对同一错误给出不同处置）
- [ ] 存量配置的升级路径明确：库中已有含未知工具名的 stage_config 时，启动行为必须**确定**
      （fail fast 报错并指明是哪个 stage 的哪个名字），不静默放行——若选择自动清理须写明理由
- [ ] `docs/agents.md` 补一条升级注意（与 `deny_unknown_fields` 那条并列），说明
      「拼错的工具名由静默忽略改为启动/写入报错」是行为变化
- [ ] `docs/testing.md` §11 或相应契约节记录该收紧；L1 / L3 各有一条用例钉住（拒绝写入 +
      启动时存量数据的确定性处置）
- [ ] 前端 `StageConfigForm` 在收到 400 时呈现后端错误信息（不吞错、不伪造成功）
- [ ] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 vitest / svelte-check / build
