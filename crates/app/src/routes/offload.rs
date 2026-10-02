//! 重活外发 GitHub 的开关（票 runner-offload/05）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/offload` | 存的状态 + **活体探测**（gh 登录态 / 外发工作流在场性） |
//! | PUT | `/offload` | 保存开关（`{enabled}`），回同样的读数（含一次新探测） |
//!
//! 与 `/rtk` 同族（决策 297）：**每次读都真探测一次**，不缓存上次结果；
//! **探测失败不拦保存**——「先开开关、后在 106 登录 gh」是共识里写明的顺序，
//! 探测读数原样摆出来，不静默成功也不静默失败。

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
    let gh = tokio::process::Command::new("gh")
        .arg("auth")
        .arg("status")
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    let (gh_authed, gh_reason) = match gh {
        Ok(out) if out.status.success() => (true, None),
        Ok(out) => (
            false,
            Some(
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("gh auth status 失败")
                    .to_string(),
            ),
        ),
        Err(e) => (false, Some(format!("gh 不在场：{e}"))),
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

/// `GET /offload`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let stored = state.store.offload_switch().await.map_err(map_core_error)?;
    let overridden = state
        .store
        .offload_switch_has_override()
        .await
        .map_err(map_core_error)?;
    let probe = probe(&state).await;
    Ok(Json(json!({
        "enabled": stored.enabled,
        // 诚实口径（决策 257）：这份状态是谁定的
        "origin": if overridden { "settings" } else { "default" },
        "probe": probe,
    })))
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
    let probe = probe(&state).await;
    Ok(Json(json!({
        "enabled": body.enabled,
        "origin": "settings",
        "probe": probe,
    })))
}
