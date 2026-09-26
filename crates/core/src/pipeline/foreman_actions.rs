//! 值班长的**直接动作面**（决策 255）：按下确认钮之后**自己动手、不经本服务接口**的那一族。
//!
//! # 判据：有没有一颗对应的界面按钮
//!
//! 提议执行共六族，分法**不是**「代码住在哪」，而是**「这颗动作有没有一个界面按钮可以对照」**：
//!
//! - **有按钮的**（`task` / `config` / `skills`）走端点处理器——「参数与界面上那颗按钮**同形**」
//!   是票 05 的硬要求（`routes/foreman.rs` 原话「不发明第二套参数语言」），**直接调 handler
//!   正是那条纪律的执行机制**。那三族留在 `crates/app`，搬走等于在这里立第二套参数语汇。
//! - **没按钮的**走本模块。
//!
//! 判据落在**动作**粒度而不是族粒度：`unstick` 住在有端点的 `task` 族里，但它自己没有端点
//! （`routes/foreman.rs` 的就地注释已明写），故它在这一面。
//!
//! 与**环境层**（决策 206）**正交**：那一维管「**能不能改**」（档位 `auto` / `ask` / `deny`），
//! 本维管「**谁来执行**」。
//!
//! # 为什么这些函数不接受 `AppState`
//!
//! 照决策 249 的纪律：新 module **接受它真正要的那几位依赖**，不伸手去拿一个大结构。
//! 四个函数各自的 interface 因此就是它们真正读的那几样——`env` 多半，`service` 只有 `store`。
//!
//! # 返回 `core::Error`，不返回 HTTP 错误
//!
//! 搬迁前这些函数返回 `ApiError`，而路由层有一条性质（「提议执行失败时，报错文案与界面上直接
//! 点那颗按钮看到的是同一句」）。归一为 `core::Error` 之后由 `map_core_error` 统一映射，于是
//! 那条性质对**按钮**与**提议**两侧**由构造保证**，不再靠重新解析一次响应状态。
//!
//! 故这里的变体选择是有意义的，不是随手挑：
//! - `Error::Validation` → 400（参数不对、动作名不存在）
//! - `Error::Conflict` → 409（情况变了：闸门没过、基准冲突、提议缺载荷）
//! - `Error::Task` → 404（台账里那个东西不在：项目不存在）

use std::path::Path;

use chrono::Duration;

use crate::agent::client::ToolCall;
use crate::config::Settings;
use crate::home::Home;
use crate::pipeline::foreman::{
    foreman_available_tools_except, foreman_tooling, ForemanMoment, FOREMAN_STAGE_KEY,
};
use crate::pipeline::repair::{finish_repair, gate_failure_note, RepairOutcome, RepairSession};
use crate::sse::SseSink;
use crate::storage::proposals::ForemanProposal;
use crate::storage::Store;
use crate::types::effective_env_mode;
use crate::{Error, Result};

/// 环境层工具的执行：与对话轮**同一个执行器**，只把确认闸关掉。
///
/// 档位**在执行时重读一次**（而不是沿用提议生成时那一份）：白名单按当前档位算，于是档位在
/// 提议之后被收紧到 `deny` 时，这条提议按不下去（报的是「不在允许集内」）。放松到 `auto`
/// 则照旧能按——收紧是安全方向，放松不是。
pub async fn run_env(
    store: &Store,
    settings: &Settings,
    home: &Home,
    sse: std::sync::Arc<dyn SseSink>,
    proposal: &ForemanProposal,
) -> Result<Option<String>> {
    let cfg = store.get_stage_config(FOREMAN_STAGE_KEY).await?;
    let env_mode = effective_env_mode(settings.env_mode, FOREMAN_STAGE_KEY, cfg.as_ref());
    // 按键执行那一趟不分级（票 07 的分级只针对自动轮）：人已经按下了那颗钮，故 `deny`
    // 为空；白名单仍按**当前**档位算一次传进去（决策 247：执行点不吃自己另筛的一份）。
    let available = foreman_available_tools_except(env_mode, &[]);
    let (tools, ctx) = foreman_tooling(
        store,
        settings,
        home,
        sse,
        &proposal.session_id,
        env_mode,
        ForemanMoment::ConfirmedPress,
        &available,
        // 也不注入托管执行者：按键那一趟根本走不到托管分支（`confirmed_once` 已短路）
        None,
        // 按下的是一个人按的动作：台账读数照人的那一轮放开（决策 291 / 票 06(a)）。
        true,
    );
    let call = ToolCall {
        id: proposal.id.clone(),
        name: proposal.tool.clone(),
        // 参数**逐字取自提议行**：这是「按下的是它当时提的那件事」的唯一凭据。
        arguments: proposal.args.to_string(),
    };
    let outcome = tools.execute(&call, &ctx).await?;
    Ok(Some(outcome.content))
}

/// 按下一条修复提议：**先 rebase 检查，再合入**。
///
/// 指纹换义（决策 212①）就落在这里：普通提议的拒执判据是「任务状态变了吗」，而修复执行的是
/// 「合入一个分支」——分支不会因为别的事变迁而失效，会变的是**基准**。故执行时先走 merge
/// 阶段已有的 `rebase_onto_with_auto_resolve`：
/// - 能干净 rebase（或自动解决冲突）→ 合入；
/// - 冲突 → **拒执**，并把冲突文件列给你（那是你要动手的地方）。
pub async fn run_repair(store: &Store, proposal: &ForemanProposal) -> Result<Option<String>> {
    let outcome = repair_outcome_of(proposal)?;
    if !outcome.gate_passed {
        return Err(Error::Conflict(format!(
            "这条修复的闸门没过，不能合入：{}",
            gate_failure_note(&outcome.gate)
        )));
    }
    let project = store
        .get_project(
            proposal
                .args
                .get("project_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        )
        .await?
        .ok_or_else(|| Error::Task("修复所属的项目不存在".into()))?;
    let repo = Path::new(&project.local_path);
    let session = RepairSession::from_outcome(&outcome, &proposal.session_id);

    // ① 基准前进 / 冲突：以「能不能干净 rebase」为准（指纹换义）
    if let crate::git::AutoRebaseOutcome::Conflict { files } = crate::git::Git
        .rebase_onto_with_auto_resolve(&session.worktree, &session.base_ref)
        .await?
    {
        return Err(Error::Conflict(format!(
            "修复分支与基准冲突（{}），没有合入——先解决这几处再按：{}",
            files.len(),
            files.join("、")
        )));
    }

    // ② 合入（与 merge 阶段同一个 git 出口）+ 回收（合入成功 → 删分支、删 worktree）
    crate::git::Git
        .merge_into_default_branch(repo, &project.default_branch, &session.branch)
        .await?;
    finish_repair(repo, &session, true).await?;
    Ok(Some(format!(
        "已合入 {} → {} 并回收修复 worktree（分支已删）",
        session.branch, project.default_branch
    )))
}

/// `service` 族（决策 210⑧ / 票 09）：**全局动作，永远只提议**。
///
/// 按下之后做的是**恢复序列**，然后**如实说清本进程没有自重启能力**——没有 supervisor 契约，
/// 擅自 `exit` 会让服务就此消失，而按下那颗钮的人未必在能把它拉起来的地方。
///
/// 这一条偏离了票面「重启服务」的字面（只重启、不改代码那件事），如实记在票 09 的收尾里：
/// 真正重启需要一条进程外的监督者，那是另一票。
pub async fn run_service(store: &Store, proposal: &ForemanProposal) -> Result<Option<String>> {
    let action = str_arg(&proposal.args, "action")?;
    if action != "restart" {
        return Err(Error::Validation(format!(
            "service 工具没有这个动作：{action}（可用：restart）"
        )));
    }
    let readings = run_recovery_sequence(store).await?;
    Ok(Some(format!(
        "已执行重启前的恢复序列：清理残留执行者 {} 个、中断的 running 任务归队 {} 个、\
         中断的项目级 run 标终态 {} 条。**本进程没有自重启能力**——请在你启动它的地方\
         （桌面壳或那个终端）重启一次，中断的任务会从归队处继续。",
        readings.cleared,
        readings.requeued.len(),
        readings.abandoned.len()
    )))
}

/// 解开一次僵死占用（决策 210⑧ / 票 09）。
///
/// 报文与托管那条（`app::runtime::StewardActions`）**不同且不该并**：一句说「已解除」、
/// 一句说「已自动解除（托管）」——那句差异说的是「谁按的」，是真的。
pub async fn run_unstick(
    store: &Store,
    settings: &Settings,
    proposal: &ForemanProposal,
) -> Result<Option<String>> {
    let task_id = str_arg(&proposal.args, "task_id")?;
    let unstuck = crate::pipeline::unstick::unstick(
        store,
        &crate::pipeline::executor::force_release,
        &task_id,
        store.now(),
        owner_stuck_window(settings),
    )
    .await?;
    Ok(Some(format!(
        "已解除僵死占用（{}）：游标 {} 转 pending（标终态的 run {:?}），现在可以 resume",
        match unstuck.kind {
            crate::storage::AttentionKind::OwnerStuck => "owner 持有超时",
            _ => "调度器处置未生效",
        },
        unstuck.cursor_id,
        unstuck.finished_runs
    )))
}

/// 「卡住」的宽限期，**唯一一处换算**。
///
/// 设置项叫 `watch_owner_stuck_minutes`（分钟），而 `unstick` 与 `stuck_evidence` 要的是
/// `chrono::Duration`。此前三个调用点各换算一次：调度器（`scheduler/mod.rs`）与托管
/// （`app::runtime`）用 `Duration::minutes`，**而提议那条用了 `Duration::seconds`**
/// ——同一个设置项、同一个判据，两条路相差 60 倍（默认值 10 时：10 秒 vs 10 分钟）。
/// 后果是同一个任务「调度器认为还没卡」而「人按下 unstick 就解开」，或反过来。
///
/// 换算收在这里之后，三处问的是同一个问题。
pub fn owner_stuck_window(settings: &Settings) -> Duration {
    Duration::minutes(settings.watch_owner_stuck_minutes as i64)
}

/// 恢复序列三步的读数（`service` 与启动时共用）。
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryReadings {
    /// 清掉的残留执行者个数。
    pub cleared: usize,
    /// 归队的中断 `running` 任务。
    pub requeued: Vec<String>,
    /// 标成终态的项目级 run。
    pub abandoned: Vec<i64>,
}

/// **恢复序列**（决策 127 / 212）：把上一轮遗留的占用与在飞的活儿收干净。
///
/// 三步、顺序有意义，两条调用路径共用**这一份实现**：
/// 1. 清 `executor_owner`（kill -9 残留）；
/// 2. 中断的 `running` 任务归队（否则调度器不接管——准入只认 `queued`）；
/// 3. 项目级 run 标终态（它既不在归队范围内、也不在 `check_timeouts` 的扫描范围内，
///    不收则跨重启永生）。
///
/// **这里只有三步，`orphan_inflight_model_requests` 不在其中**（决策 255④）：那一步是
/// **启动特有**的——它自己的 doc 就是「启动时把**上一个实例留下的**在飞请求收成终态」，
/// 判据 `finished_at IS NULL` 没有进程限定。而**运行中**被丢弃的请求另有承担者：
/// `crate::agent::recording::Settle` 的 `Drop` 以 `ABANDONED_NOTE` 收成 `Timeout`。
/// 故序列是 **3+1**：本函数三步，启动时那一步留在调用点。
pub async fn run_recovery_sequence(store: &Store) -> Result<RecoveryReadings> {
    let cleared = store.clear_executor_owners().await?;
    let requeued = store.requeue_running_tasks().await?;
    let abandoned = store.abandon_stale_project_runs().await?;
    Ok(RecoveryReadings {
        cleared,
        requeued,
        abandoned,
    })
}

/// 从提议里读回一次修复的现场。
///
/// 载荷是**必写**的（落库时不设 TTL 的那条提议一定带 `payload`），故缺载荷与读不出来都是
/// 「不该发生」——以 `Conflict` 报出而不是 `internal`：这两种情况对按下那颗钮的人来说，
/// 语义都是「这条提议现在执行不了」。
fn repair_outcome_of(proposal: &ForemanProposal) -> Result<RepairOutcome> {
    let payload = proposal
        .payload
        .as_ref()
        .ok_or_else(|| Error::Conflict("修复提议缺载荷（不该发生：落库时必写）".into()))?;
    serde_json::from_value(payload.clone())
        .map_err(|e| Error::Conflict(format!("修复提议的载荷读不出来：{e}")))
}

fn str_arg(args: &serde_json::Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Validation(format!("这条提议缺少参数 {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 换算的唯一性：设置项的**分钟**语义不许再被按秒读一次。
    ///
    /// 这条正是搬移过程中发现的缺陷的回归——`run_unstick` 此前用 `Duration::seconds`，
    /// 而调度器与托管用 `Duration::minutes`，同一个判据差 60 倍。
    #[test]
    fn owner_stuck_window_reads_the_setting_as_minutes() {
        let settings = Settings {
            watch_owner_stuck_minutes: 10,
            ..Default::default()
        };
        assert_eq!(owner_stuck_window(&settings), Duration::minutes(10));
        assert_eq!(owner_stuck_window(&settings).num_seconds(), 600);

        let zero = Settings {
            watch_owner_stuck_minutes: 0,
            ..Default::default()
        };
        assert_eq!(owner_stuck_window(&zero), Duration::zero());
    }

    #[test]
    fn missing_arg_is_a_validation_error() {
        let err = str_arg(&serde_json::json!({"action": "  "}), "action").unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "{err}");
    }
}
