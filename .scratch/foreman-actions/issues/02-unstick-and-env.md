# 02: 搬 `unstick` 与 `env` 两族，并删 `force_release` 转发

**What to build:** 把两族搬进 `pipeline/foreman_actions.rs`：

- **`unstick`**（现 `foreman.rs:731-753`，住在 `run_task_tool` 的 `task` 族里）
  → `run_unstick(store, settings, proposal) -> Result<Option<String>>`。
  报文逐字照搬（「已解除僵死占用（{kind}）：游标 {cursor_id} 转 pending（标终态的 run {runs}），
  现在可以 resume」）。
- **`env` 三件**（现 `run_env_tool`，`:482-523`）
  → `run_env(store, settings, home, sse, proposal) -> Result<Option<String>>`。

**`force_release` 转发删除**：`crates/app/src/runtime.rs:152-154` 是到
`core::pipeline::executor::force_release` 的一行纯转发（后者 `pub`、签名相同）。
删掉它，`foreman.rs` 与 `runtime.rs:93` 两处改直接引 core 的。

**`unstick` 的两处调用从此共用同一个 core 实现**（`foreman.rs:736` 提议按、
`runtime.rs:93` 托管），但**报文不同照旧**——一句说「已解除」、一句说「已自动解除（托管）」，
那句差异是真的，不并（决策 255⑥）。

**Blocked by:** 01（先把 module 与搬迁模式立起来）。

**Status:** done（2026-09-23）

- [x] `run_unstick` 落地；`foreman.rs` 的 `task` 族那一臂改成一行 core 调用
- [x] `run_env` 落地；**带上 `189d3cc` 的改动**——`foreman_tooling` 的 `available` 参数取
      `foreman_available_tools_except(env_mode, &[])` 算出来的那份，不是 `&[]`
      （决策 247；照搬现状，不要照搬旧代码）
- [x] `runtime.rs` 的 `force_release` 转发删除；两处调用改引
      `agentpipeline_core::pipeline::executor::force_release`
- [x] 既有 `env_mode` 测试（`crates/core/tests/integration/env_mode.rs` 的 `ConfirmedPress`
      那组）仍绿——它们是这次搬迁唯一的回归网
- [x] `make check` 绿

## Comments

- **`unstick` 为什么算「直接动作面」而不算 `task` 族**：判据是动作粒度上的「有无对应按钮」。
  `foreman.rs:733` 的就地注释已经这么写了，本票只是让代码结构跟上那句话（决策 255②）。
- **档位重读不能丢**：`run_env_tool` 在按下时**重新读一次 `env_mode`**，且这是刻意的安全属性
  （`:480-481` 的注释：「收紧是安全方向，放松不是」）。搬进 core 时这段语义逐字保留——
  建议由 `run_env` 自己读 `store.get_stage_config(FOREMAN_STAGE_KEY)` 后调 `effective_env_mode`。
