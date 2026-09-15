//! 技能市场：本地导入 / 目录扫描 / 卸载（决策 172⑤，票 09）。
//!
//! 五个端点，覆盖票 09 的全部验收项：
//!
//! | 端点 | 作用 |
//! |---|---|
//! | `GET /skills` | 已安装技能清单（名字 + 描述 + 来源 + 被哪些阶段/节点引用） |
//! | `POST /skills/import?name=&overwrite=` | **上传 zip**（请求体是 zip 原始字节） |
//! | `POST /skills/import-dir` | **目录导入**，支持批量（逐项结果，不中断整批） |
//! | `GET /skills/scan?root=` | 扫描一个本地技能根（`~/.zcode/skills` 之类） |
//! | `DELETE /skills/{name}` | 卸载技能 |
//!
//! ## 为什么 zip 走原始字节而不是 multipart / base64
//!
//! 三者都能传文件，选最省的那个：multipart 要给 axum 开 `multipart` 特性（新引入
//! `multer` 一棵树），base64 要一个编解码依赖并让体积涨 33%，而**原始字节零依赖**——
//! `fetch(url, {method: "POST", body: file})` 直接就发了。名字与是否覆盖放在 query 里
//! （它们不是包的内容，是**这次导入操作**的参数）。
//!
//! ## 卸载不检查引用
//!
//! 卸载一个仍被阶段配置引用的技能**是允许的**——引用完整性由启动校验与 `PUT /stage-configs`
//! 的 fail fast 兜住（票 09 的显式要求：技能名是唯一身份，不得静默降级）。把引用检查塞进
//! 卸载会制造隐蔽的先后依赖：想卸载得先改配置、想改配置得先卸载。`GET /skills` 的
//! `declared_in` 字段负责把「哪些配置会因此报错」提前告诉界面。

use agentpipeline_core::agent::skill_import::{self, ScanEntry, SkillPackage};
use agentpipeline_core::agent::skills::{discover, SkillSource};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// 上传体积上限（64 MiB）。
///
/// `skill_import` 内部对**解压后**的每个条目另有 16 MiB 上限——那道才是真正防解压炸弹的。
/// 这道只是避免把一个超大请求体读进内存；64 MiB 对「一个 markdown 技能目录的 zip」有
/// 三个数量级余量（上游最大的技能约 12 KiB）。
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/skills", get(list))
        .route(
            "/skills/import",
            // 默认请求体上限是 2 MB，对技能包不够——本路由单独放宽
            post(import_zip).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/skills/import-dir", post(import_dir))
        .route("/skills/scan", get(scan))
        .route("/skills/{name}", delete(uninstall))
        .with_state(state)
}

// ─────────────────────────────── GET /skills ───────────────────────────────

/// `GET /skills`：已安装技能的清单。
///
/// `declared_in` 是这个端点相对于「只列目录」的全部价值：它回答「卸载 X 会让哪些配置报错」。
/// 票 09 允许卸载仍被引用的技能（引用完整性由启动校验兜住），因此界面必须在动手前看得到后果。
pub async fn list(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let root = state.home.skills_dir();
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;

    let skills: Vec<serde_json::Value> = discover(&root)
        .into_iter()
        .map(|s| {
            // 两类来源只在 `kind` 与 `path` 上不同（工具型技能在 PATH 里，没有技能根下的文件）
            let (kind, path) = match s.source {
                SkillSource::Markdown { path } => ("markdown", json!(path.display().to_string())),
                SkillSource::Tool => ("tool", serde_json::Value::Null),
            };
            json!({
                "name": s.name,
                "kind": kind,
                "description": s.frontmatter.description,
                "disable_model_invocation": s.frontmatter.disable_model_invocation,
                "path": path,
                "declared_in": declared_in(&configs, &s.name),
            })
        })
        .collect();

    Ok(Json(json!({
        "skills": skills,
        "skills_root": root,
    })))
}

/// 该技能名被哪些阶段级 / 节点级配置引用（`["阶段 architect-design", "阶段 test 节点 execute"]`）。
fn declared_in(configs: &[agentpipeline_core::types::StageConfig], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for cfg in configs {
        for (where_, decl) in agentpipeline_core::config::declared_skill_decls(cfg) {
            if decl.name == name && !out.contains(&where_) {
                out.push(where_);
            }
        }
    }
    out
}

// ─────────────────────────── POST /skills/import ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct ImportQuery {
    /// 技能名。可推断（包里有 `{name}/SKILL.md`）时可省；平铺打包的包必须给。
    #[serde(default)]
    pub name: Option<String>,
    /// 同名覆盖的**显式确认**（票 09：默认拒绝）。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /skills/import`：上传一个 zip 包。
///
/// 请求体是 zip 的**原始字节**（不是 multipart、不是 base64——理由见模块头注释）。
pub async fn import_zip(
    State(state): State<AppState>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> ApiResult<impl IntoResponse> {
    if body.is_empty() {
        return Err(ApiError::bad_request(
            "请求体为空：请发送 zip 文件的原始字节",
        ));
    }
    let package = SkillPackage::from_zip(&body, q.name.as_deref()).map_err(map_core_error)?;
    finish_import(&state, &package, q.overwrite)
}

// ───────────────────────── POST /skills/import-dir ─────────────────────────

#[derive(Debug, Deserialize)]
pub struct ImportDirBody {
    /// 待导入的技能目录（每个目录自身含 `SKILL.md`）。支持批量。
    pub paths: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /skills/import-dir`：从一个或多个本地技能目录导入（票 09 的批量导入）。
///
/// **逐项返回结果**，一项失败不中断整批：用户从 `~/.zcode/skills` 里勾了十个技能，其中一个
/// 恰好残缺，不该让另外九个都装不上。
pub async fn import_dir(
    State(state): State<AppState>,
    Json(body): Json<ImportDirBody>,
) -> ApiResult<impl IntoResponse> {
    if body.paths.is_empty() {
        return Err(ApiError::bad_request("paths 为空：请至少给出一个技能目录"));
    }
    let root = state.home.skills_dir();
    let results = skill_import::install_batch(&root, &body.paths, body.overwrite);

    let items: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(path, result)| match result {
            Ok(info) => json!({
                "path": path,
                "ok": true,
                "name": info.name,
                "description": info.description,
                "sibling_count": info.sibling_count,
            }),
            Err(e) => json!({
                "path": path,
                "ok": false,
                "error": e.to_string(),
                // 同名冲突可由界面提示「是否覆盖后重试」
                "conflict": matches!(e, agentpipeline_core::Error::Conflict(_)),
            }),
        })
        .collect();

    let succeeded = items.iter().filter(|i| i["ok"] == true).count();
    Ok(Json(json!({
        "results": items,
        "succeeded": succeeded,
        "failed": items.len() - succeeded,
    })))
}

fn finish_import(
    state: &AppState,
    package: &SkillPackage,
    overwrite: bool,
) -> ApiResult<Json<serde_json::Value>> {
    let root = state.home.skills_dir();
    let info = skill_import::install(&root, package, overwrite).map_err(map_core_error)?;
    Ok(Json(json!({
        "skill": {
            "name": info.name,
            "description": info.description,
            "sibling_count": info.sibling_count,
        }
    })))
}

// ──────────────────────────── GET /skills/scan ────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ScanQuery {
    /// 要扫描的技能根（如 `~/.zcode/skills`）。`~` 会被展开。
    pub root: String,
}

/// `GET /skills/scan?root=`：列出一个本地技能根下的全部可导入技能。
///
/// 返回名字 + 描述 + **是否已存在**（已存在的导入会撞同名冲突，界面据此提示覆盖）。
pub async fn scan(
    State(state): State<AppState>,
    Query(q): Query<ScanQuery>,
) -> ApiResult<impl IntoResponse> {
    let raw = q.root.trim();
    if raw.is_empty() {
        return Err(ApiError::bad_request("root 为空：请给出要扫描的技能根目录"));
    }
    let source = resolve_scan_root(raw);
    if !source.is_dir() {
        return Err(ApiError::bad_request(format!(
            "扫描目录不存在或不是目录：{}",
            source.display()
        )));
    }
    // 只读：列目录 + 读 SKILL.md。**不落盘**，故不校验「是否在技能根内」——
    // 扫描的是用户指定的来源，不是写入目标。
    let entries =
        skill_import::scan_root(&source, Some(&state.home.skills_dir())).map_err(map_core_error)?;

    let skills: Vec<serde_json::Value> = entries.iter().map(scan_entry_json).collect();
    Ok(Json(json!({
        "root": source,
        "skills": skills,
    })))
}

fn scan_entry_json(e: &ScanEntry) -> serde_json::Value {
    json!({
        "name": e.name,
        "description": e.description,
        "exists": e.exists,
        "path": e.path,
    })
}

/// 展开 `~` 后按绝对路径使用（与 `resolve_config_path` 同口径，但不需要 home 根兜底：
/// 扫描对象是用户给的外部目录，相对路径按当前进程工作目录即可，这里统一要求绝对路径）。
fn resolve_scan_root(raw: &str) -> std::path::PathBuf {
    if raw == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(raw)
}

// ────────────────────────── DELETE /skills/{name} ──────────────────────────

/// `DELETE /skills/{name}`：卸载技能。
///
/// 技能不存在 → 404；工具型技能（PATH 可执行文件）→ 400（删用户的 PATH 文件是灾难）。
/// 仍被引用的技能**可以卸载**，见模块头注释。
pub async fn uninstall(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let root = state.home.skills_dir();
    skill_import::uninstall(&root, &name).map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "name": name })))
}
