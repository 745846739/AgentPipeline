# 提议执行从路由层归位：值班长的「直接动作面」收进 core（foreman-actions）

**状态（2026-09-23）：票 01–04 全部落地（`Status: done`），`make check` 四段分段全绿**（闸门取证与跨会话 e2e 撞车记录见决策 255 的落地附注）。 本目录来自一次架构评审
（`improve-codebase-architecture`）的候选 6，经**三轮拷问 Q1–Q8 全部同意**定下范围、接口与测试。
评审报告是临时的，不入库；设计结论以本文件与**决策 255** 为准。

同一轮评审的候选 1 / 2 / 3 / 4 / 5 / 7 已由决策 245–250 落地（`b7107bb`→`72bb715`），
候选 8 / 10 由决策 252–254 与 251 另行落设计（`.scratch/mirror-contract/`、`.scratch/talk-judgments/`）。
**本目录只处理候选 6**，且落地前已确认 `crates/app/src/routes/foreman.rs` 自 `94fb6ee` 以来
**未被那批改动碰过**（唯一一次是 `189d3cc` 把 `run_env_tool` 的 `&[]` 换成 `available`，见
§四的「与 247 的交点」）。

## 一、问题

`crates/app/src/routes/foreman.rs` 的 `run_proposal_tool`（`:456-475`）是一条按工具名的分派器，
六个族各有一个执行函数，**约 445 行住在路由层**（`:433-903`）。这些代码不是 transport：

| 族 | 函数 | 读取的 `AppState` 字段 | 实际调用 |
|---|---|---|---|
| env（`write_file`/`edit_file`/`run_command`） | `run_env_tool` `:482-523` | `store` / `settings` / `home` / `sse` | `foreman_tooling` + `ToolExecutor::execute`（全 core） |
| repair | `run_repair_proposal` `:534-599` | **只有 `store`** | `git::Git` + `pipeline::repair`（全 core） |
| service | `run_service_tool` `:611-643` | **只有 `store`** | 三个 `Store` 方法 |
| unstick（住在 `task` 族里） | `run_task_tool` 的一臂 `:731-753` | `store` / `settings` | `pipeline::unstick::unstick`（core） |

**后果是测试真空**：`run_repair_proposal` 与 `run_service_tool` 在 **app 与 core 两侧都零测试**
（全仓 grep 确认——`crates/app/tests/` 里 `repair` / `service` 零命中）。它们只能经
`POST /foreman/proposals/{id}/execute` 打到一个真 git 仓 + 真 store 上才够得着，而分派是个私有
`match`、拿 `&AppState`，没有可注入的缝。

## 二、判据（一条，防止复发）

**判据是「有没有一颗对应的界面按钮」，不是「代码住在哪」。**

- **有按钮的**（`task` / `config` / `skills`）走端点处理器——「参数与界面上那颗按钮**同形**」
  是票 05 的硬要求（`foreman.rs:649` 原话「不发明第二套参数语言」），**直接调 handler 正是这条
  纪律的执行机制**。搬走它们等于在 core 立第二套参数语汇，恰好是决策 207④ 要防的事。
- **没按钮的**（env 三件 / `unstick` / `repair` / `service`）走新 module。

判据落在**动作粒度**而非族粒度：`unstick` 住在有端点的 `task` 族里，但它自己没有端点——
`foreman.rs:733` 的就地注释已明写「`unstick` 不在端点里（它是修补动作面，不是界面上的按钮面）」。

## 三、形状

新建 `crates/core/src/pipeline/foreman_actions.rs`，**一族一个具名函数**：

```rust
pub async fn run_env(store: &Store, settings: &Settings, home: &Home,
                     sse: Arc<dyn SseSink>, proposal: &ForemanProposal)
    -> Result<Option<String>>;

pub async fn run_repair(store: &Store, proposal: &ForemanProposal)
    -> Result<Option<String>>;

pub async fn run_service(store: &Store, proposal: &ForemanProposal)
    -> Result<Option<String>>;

pub async fn run_unstick(store: &Store, settings: &Settings, proposal: &ForemanProposal)
    -> Result<Option<String>>;
```

**不设「一个入口内部分派」**——那会让工具名语汇出现第二份副本，正是决策 247 修掉的病
（前端那份曾漂成 18/21）。分派器 `run_proposal_tool` **留在 app 一处**，六条臂仍在一张表里，
其中四条各是一行 core 调用。各函数**接受它真正要的那几位**，照决策 249 的纪律「不伸手拿
`&Executor`」。

## 四、四件必须一起记的事实

### 1. 恢复序列是三步，不是四步

`run_service` 做三步——`clear_executor_owners` → `requeue_running_tasks` →
`abandon_stale_project_runs`，与 `serve.rs:379-393` 共用**同一个 core 函数**（`serve.rs` 保留
自己的日志）。

**第四步 `orphan_inflight_model_requests` 不并进来**：它是**启动特有**的
（`storage/model_requests.rs:337-347` 的 doc 即「启动时把**上一个实例留下的**在飞请求收成终态」，
`WHERE finished_at IS NULL` 无进程限定），而**运行中**被丢弃的请求另有承担者——
`agent/recording.rs` 的 `Settle` 守卫（`:190-210`）在 `Drop` 里以 `ABANDONED_NOTE` 收成 `Timeout`。
**故序列是 3+1**：模块收三步，`serve.rs` 把第四步留在启动调用点。

> **订正记账（评审第 2 轮的一处错误答案）**：拷问时我（评审方）主张四步全并，理由是「运行中
> 同样正确且必要」。该主张**不成立**——启动语义写在它自己的 doc 里，且运行中有 `Settle::drop`
> 兜底。用户当时选的选项 (iii)「3+1」才是对的。此订正已入决策 255④。

### 2. `endpoint_json` 不是「假传输边界」

评审报告称 `endpoint_json`（`:916-943`）是「手搓的进程内 HTTP 边界」——**不成立**。那里没有
HTTP、没有路由跳：`run_task_tool` 是普通 Rust 函数调用（`tasks::create(State(...), Json(...))`），
`endpoint_json` 只是把 handler 已算好的 `ApiResult<R>` 经 `into_response()` +
`axum::body::to_bytes` 读回来压成一行。它是**序列化往返**，不是 transport。

「丢了 `detail` / `kind`」也**当前是理论的不是真实的**：全仓只有三处设置这两个字段
（`market.rs:385,389`、`skills.rs:772`），全在 `/market/install` 与 `/skills/install` 推荐路径上，
**都不在提议可达图内**。代价真实但要如实记账，不夸大。

### 3. 归一为 `core::Error` 让「同一句话」由构造保证

搬走的三族返回 `core::Error`、由 `map_core_error` 统一映射，于是「按钮」与「提议」两侧对同一个
失败给出同一句**由构造保证**，比今天靠重新解析 status 更可靠。

`foreman.rs:905-908` 与 `:454-455` 那条「端点的错误逐字带回」**对留在原地的三族仍然有效**，
对搬走的三族本就空洞（它们没有端点可对照）。

### 4. 与决策 247 的交点（落地前必读）

`189d3cc` 改过 `run_env_tool` 一处：`foreman_tooling` 的 `available` 参数从 `&[]` 换成
`let available = foreman_available_tools_except(env_mode, &[]);` 算出来的那份。搬迁时**必须带上
这个改动**（照搬 `189d3cc` 之后的现状，不要照搬决策 255 写下之前的旧代码）。

## 五、明确不做

- **不搬有端点的三族**（`task` / `config` / `skills`）。除 §二 的理由外，`tasks::retry`（约 90 行）
  与 `stage_configs::put`（约 70 行）的搬移要动 `core::Error` 给它加 `NotFound` / `Unavailable`
  变体（它现在没有；`map_core_error` 只把 `Task` / `Cursor` 映到 404），代价与收益不成比例。
- **不动 `execute_proposal`（`:289-382`）那 94 行**。它已有三条测试（一次一按 / 过期 / 态势拒执：
  `api_contract.rs:5157`、`:5120`、`:5224`），且必须能分派到**六个**族（三个留在 app），
  硬搬会迫使它跨 crate 反向调用。
- **不拆 `StewardActionRunner`**（`agent/tools.rs:229-235`）。它与 `for_foreman` 同姿态：既有替换点
  上的一个槽位，且「不注入 = 不放行」承担授权语义。事实核对显示它的实现其实**不依赖任何
  app-only 能力**（`ResumeHook` 与 core 的 `ResumeFn` 结构同形，`force_release` 在 core 是 `pub`），
  故它是**可拆的**——但那是另一票的事，不该混进这次搬迁。
- **不给 `unstick` 换新报文**。`foreman.rs:745` 说「已解除僵死占用」、`runtime.rs:99` 说
  「已自动解除僵死占用（托管）」——那句差异是真的，不并。

## 六、验收

- 既有测试**一条不删、断言不放宽**（照决策 249）。
- 新增 service 契约用例（提议按下去 → 三个 `Store` 方法真跑 → 归队读数可断言）与 repair 契约
  用例（带载荷的 `seed_proposal` 变体 + 真仓库上的分支/worktree → 干净 rebase 则合入、
  冲突则拒执并列出文件）。
- 每票收口 = 该票测试全绿 + `make check` 绿（决策 168 的唯一权威闸门）。
