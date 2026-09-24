# 01: 新建 `pipeline/foreman_actions.rs` 并搬 service 族（含三步恢复序列）

**What to build:** 新建 `crates/core/src/pipeline/foreman_actions.rs`，把 `run_service_tool`
（`crates/app/src/routes/foreman.rs:611-643`）搬进来成为 `run_service(store, proposal)`，
返回 `Result<Option<String>>`。

**恢复序列**：三步的共用实现也落在本 module（`pub async fn run_recovery_sequence(store) -> Result<RecoveryReadings>`），
`serve.rs:379-393` 改调它、保留自己的日志。**第四步 `orphan_inflight_model_requests` 不并进来**
（决策 255④：它是启动特有的，运行中被丢弃的请求由 `agent/recording.rs` 的 `Settle` 守卫兜底）。

**order**：本票先做，因为它是四族里唯一**零额外基建**的——`run_service` 只碰三个 `Store` 方法，
`api_with_foreman` harness 直接够用，测试可以同票交付。

**Blocked by:** 无。

**Status:** done（2026-09-23）

- [x] `crates/core/src/pipeline/foreman_actions.rs` 落地：`run_service(store, proposal) -> Result<Option<String>>`
- [x] 三步恢复序列收成 core 的一个函数；`serve.rs` 改调它（日志留在 `serve.rs`，第四步也留在 `serve.rs`）
- [x] `crates/core/src/pipeline/mod.rs` 声明 `pub mod foreman_actions;`
- [x] `foreman.rs` 的 `"service"` 臂改成一行 core 调用；报文逐字不变
      （「已执行重启前的恢复序列：清理残留执行者 N 个…**本进程没有自重启能力**…」）
- [x] 契约用例：落一条 `service` 提议（`seed_proposal` 原样可用，service 不需要载荷）→
      `POST /foreman/proposals/{id}/execute` → 断言三个 `Store` 方法真的跑了
      （种一个 `running` 且带 `executor_owner` 的任务，执行后它归队 `queued`、owner 被清）
- [x] `make check` 绿

## Comments

- **为何 service 先行**（决策 255⑦）：repair 要真 git 仓 + 手搭 `RepairOutcome` 载荷，且
  `seed_proposal`（`api_contract.rs:5062`）硬编码 `payload: None` / `kind: ApiCall`，得先补一个
  带载荷的兄弟助手；service 什么额外基建都不要。先落 service 验证搬迁模式本身，repair 随后。
- **报文里的 `{}` 个数**：现在那句 `format!` 有四个占位（cleared 是 `{cleared}`，
  requeued/abandoned 各两个调用）。搬运时**逐字照搬**，包括 `requeued.len()` 与
  `abandoned.len()` 的用法——不要顺手改写成别的读法。
