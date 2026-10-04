//! 重活外发 GitHub 的开关（票 runner-offload/05）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/offload` | 存的状态 + **活体探测**（gh 登录态 / 外发工作流在场性）+ 最近一次链路失败读数 |
//! | PUT | `/offload` | 保存开关（`{enabled}`），回同样的读数（含一次新探测） |
//!
//! 与 `/rtk` 同族（决策 297）：**每次读都真探测一次**，不缓存上次结果；
//! **探测失败不拦保存**——「先开开关、后在 106 登录 gh」是共识里写明的顺序，
//! 探测读数原样摆出来，不静默成功也不静默失败。
//!
//! 读数外壳（`enabled` / `origin` / `probe` / `last_failure_at`）由模块内私有的
//! `readout` 帮手两端点共用，GET 与 PUT 的读数同形因此是**结构保证**而不是纪律。
//! `probe` 的活体探测语义（10s 超时、失败不拦保存）仍在 `probe` 里，本帮手只是按次序调用它。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiResult, AppState};

/// 活体探测：gh 登录态 + 外发工作流在场性。两项独立报告，谁缺谁补齐。
///
/// `gh auth status` 是一条真子进程（~50ms 级）；工作流在场性按**约定路径**查
/// 每个已登记项目的 `.github/workflows/offload.yml`——与 deploy 闸门用的
/// 「仓库里有没有这份文件」同一口径，不做网络往返。
async fn probe(state: &AppState) -> serde_json::Value {
    // `gh auth status` 会做**真网络校验**（token 有效性、SSH 探测）——出口被黑洞的机器
    // （106 恰是这种形态）上能挂几分钟。探测是设置页的读数，不是任务收口的闸门：
    // 10 秒拿不到答案就按「未登录」上报，超时本身写进 reason 原样摆出来。
    let gh = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("gh")
            .arg("auth")
            .arg("status")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let (gh_authed, gh_reason) = match gh {
        Ok(Ok(out)) if out.status.success() => (true, None),
        Ok(Ok(out)) => (
            false,
            Some(
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("gh auth status 失败")
                    .to_string(),
            ),
        ),
        Ok(Err(e)) => (false, Some(format!("gh 不在场：{e}"))),
        Err(_) => (false, Some("探测超时（10s）——出口可能不通".to_string())),
    };
    let workflow_present = workflow_present_in_any_project(state).await;
    json!({
        "gh_authed": gh_authed,
        "gh_reason": gh_reason,
        "workflow_present": workflow_present,
    })
}

/// 任一已登记项目里有 `offload.yml` 即算在场（外发是对仓库的操作，项目只是入口）。
async fn workflow_present_in_any_project(state: &AppState) -> bool {
    let Ok(projects) = state.store.list_projects().await else {
        return false;
    };
    projects.iter().any(|p| {
        std::path::Path::new(&p.local_path)
            .join(".github/workflows/offload.yml")
            .exists()
    })
}

/// 两个端点共用的读数外壳（GET/PUT 同形的唯一来源）。
///
/// `enabled` 与 `origin` 由调用方给——那是两端点**唯一**不同的两个事实：
/// GET 报库里已存的值与它的来路（保存过 = settings，否则 default），
/// PUT 报本次请求的值与 settings。其余两格（`probe` 的活体探测、
/// `last_failure_at` 的链路失败读数）在这里拼一次，两端点天然同形。
///
/// 本帮手收的是**读数外壳**；`probe` 的活体探测语义（10s 超时、失败不拦保存）
/// 仍在 `probe` 里，本帮手只是按次序调用它。
async fn readout(state: &AppState, enabled: bool, origin: &str) -> ApiResult<serde_json::Value> {
    let last_failure = state
        .store
        .offload_last_failure()
        .await
        .map_err(map_core_error)?;
    let probe = probe(state).await;
    Ok(json!({
        "enabled": enabled,
        // 诚实口径（决策 257）：这份状态是谁定的
        "origin": origin,
        "probe": probe,
        // 最近一次**链路**失败（票 08）：null = 从没失败过（读数「无」）。
        // 远端命令跑红不写这列，外发成功一轮即清。
        "last_failure_at": last_failure,
    }))
}

/// `GET /offload`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let stored = state.store.offload_switch().await.map_err(map_core_error)?;
    let overridden = state
        .store
        .offload_switch_has_override()
        .await
        .map_err(map_core_error)?;
    let readout = readout(
        &state,
        stored.enabled,
        if overridden { "settings" } else { "default" },
    )
    .await?;
    Ok(Json(readout))
}

#[derive(Debug, Deserialize)]
pub struct OffloadSwitchBody {
    pub enabled: bool,
}

/// `PUT /offload`：保存开关，回同一份读数。
///
/// 保存即活：外发工具每条命令懒读这一行（票 06），下一**条**命令就按新值走。
pub async fn set_enabled(
    State(state): State<AppState>,
    Json(body): Json<OffloadSwitchBody>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .store
        .set_offload_switch(body.enabled)
        .await
        .map_err(map_core_error)?;
    let readout = readout(&state, body.enabled, "settings").await?;
    Ok(Json(readout))
}
