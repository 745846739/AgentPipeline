//! 命令执行走 rtk 的开关（决策 297 / 票 03、05）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/rtk` | 存的状态 + **活体探测**（路径 / 版本 / 可用 / 原因） |
//! | PUT | `/rtk` | 保存开关（`{enabled, path?}`），回同样的读数（含一次新探测） |
//!
//! **每次读都真探测一次**（不缓存上次结果）：`lanToggle` 那条纪律——重读目标态才算数，
//! 决策 257 的学费是「漏读」交的。探测只读文件系统与一条子进程，代价 ~45ms 级，
//! 而这个页面是设置页，不是热路径。
//!
//! **探测失败不拦保存**（决策 297）：一个输出优化器不该有权限拦人，而开发机上
//! 「先开开关、后装二进制」是常见顺序。两个结果都是 200，差别只在 `probe.available`。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiResult, AppState};

/// 探测结果 → JSON（两种响应共用，保证 GET 与 PUT 的读数同形）。
fn probe_json(probe: &agentpipeline_core::rtk::Availability) -> serde_json::Value {
    json!({
        "available": probe.available,
        "path": probe.path.as_ref().map(|p| p.display().to_string()),
        // 这一份路径是谁定的：手填 / 服务进程 PATH / 已知目录
        "source": probe.source.map(|s| s.as_str()),
        "version": probe.version,
        // 三种失败各有各的说法，界面原样摆出来（不假装可用）
        "reason": probe.reason,
    })
}

/// `GET /rtk`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let stored = state.store.rtk_switch().await.map_err(map_core_error)?;
    let overridden = state
        .store
        .rtk_switch_has_override()
        .await
        .map_err(map_core_error)?;
    let probe = agentpipeline_core::rtk::probe(stored.path.as_deref()).await;
    Ok(Json(json!({
        "enabled": stored.enabled,
        // 诚实口径（决策 257）：这份状态是谁定的
        "origin": if overridden { "settings" } else { "default" },
        "path": stored.path.as_ref().map(|p| p.display().to_string()),
        "probe": probe_json(&probe),
    })))
}

#[derive(Debug, Deserialize)]
pub struct RtkSwitchBody {
    pub enabled: bool,
    /// 手填的兜底路径（可选）。`None` / 空串 = 回到自动解析。
    #[serde(default)]
    pub path: Option<String>,
}

/// `PUT /rtk`：保存开关，回同一份读数。
///
/// 保存即活：`CommandRunner` 每条命令懒读这一行（`RtkSource::Store`），下一**条**命令
/// 就按新值走——不必重启，也不打断已经在跑的那条。
pub async fn set_enabled(
    State(state): State<AppState>,
    Json(body): Json<RtkSwitchBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = body
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(std::path::Path::new);
    state
        .store
        .set_rtk_switch(body.enabled, path)
        .await
        .map_err(map_core_error)?;
    let probe = agentpipeline_core::rtk::probe(path).await;
    Ok(Json(json!({
        "enabled": body.enabled,
        "origin": "settings",
        "path": path.map(|p| p.display().to_string()),
        "probe": probe_json(&probe),
    })))
}
