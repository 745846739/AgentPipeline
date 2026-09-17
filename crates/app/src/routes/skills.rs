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
//! 票 11 再加三个端点（装前预览与信任标记）：
//!
//! | 端点 | 作用 |
//! |---|---|
//! | `GET /skills/{name}/preview` | 已安装技能的三项预览 |
//! | `POST /skills/preview?name=` | **装前**预览：同一个包还没落盘，先看三项 |
//! | `PUT /skills/{name}/trust` | 显式信任 / 撤销信任（改写引用它的阶段配置） |
//!
//! ## 预览是告知，不是准入
//!
//! 第 ③ 项（正文特征扫描）**没有任何拒绝路径**：命中 `curl` 或 `.env` 不会挡住安装。
//! 正则既拦不住变形又会误伤合法技能（`rtk` 正文里有 `curl` 字样），把风险判定交给它只会
//! 制造假安全。它的价值是让用户在按下确认前**看见事实**——市场环境下摘要校验只证明
//! 「没被改过」，证明不了「内容是善意的」。
//!
//! ## 信任是声明的一个字段，不是第二份账
//!
//! `trusted` 落在票 05 的 `SkillDecl` 上，写入侧已有门（未信任 + `full` 被拒）。本端点的
//! 「信任此技能」是**改写引用它的那些声明**，而不是另建一张信任表——后者会让「这个技能到底
//! 可不可信」出现两个答案，而这是安全相关判定（未信任不得全文注入），两个答案意味着其中
//! 一条路径必然判错。
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

use agentpipeline_core::agent::skill_import::{self, PackageInfo, ScanEntry, SkillPackage};
use agentpipeline_core::agent::skill_preview::{self, FeatureKind, FeatureScan};
use agentpipeline_core::agent::skills::{discover, SKILL_FILE};
use agentpipeline_core::types::StageConfig;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::str::FromStr;

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
        .route(
            "/skills/preview",
            // 与 import 同源：请求体可能是整个技能包的 zip 字节
            post(preview_package).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/skills/recommendations", get(recommendations))
        .route("/skills/install", post(install_recommended))
        .route("/skills/{name}/preview", get(preview))
        .route("/skills/{name}/trust", put(set_trust))
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
            // 技能只有一个来源（技能根下的 markdown，决策 185），故 `kind` 字段已退场：
            // 一个恒为 "markdown" 的判别位只会让读契约的人以为还有别的可能。
            json!({
                "name": s.name,
                "description": s.frontmatter.description,
                "disable_model_invocation": s.frontmatter.disable_model_invocation,
                "path": s.path.display().to_string(),
                // 算法在 core 里（`config::declared_skill_where`）：值班长的 `read_skills`
                // 工具读的是同一份，两处各写一遍会漂移。
                "declared_in": agentpipeline_core::config::declared_skill_where(&configs, &s.name),
            })
        })
        .collect();

    Ok(Json(json!({
        "skills": skills,
        "skills_root": root,
    })))
}

/// 该技能名被哪些阶段级 / 节点级配置引用（`["阶段 architect-design", "阶段 test 节点 execute"]`）。
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
/// 技能不存在 → 404。
/// 仍被引用的技能**可以卸载**，见模块头注释。
pub async fn uninstall(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let root = state.home.skills_dir();
    skill_import::uninstall(&root, &name).map_err(map_core_error)?;
    // 来源记录一并删（决策 194）：留着的话，下一次同名安装的冲突报文会报一个**已经不存在**
    // 的技能曾经从哪儿来。停用 / 启用（阶段配置里的引用）与记录无关，不联动。
    state
        .store
        .forget_skill_source(&name)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "name": name })))
}

// ───────────────── 装前预览与信任标记（决策 172④⑤，票 11）─────────────────

/// 尚不存在声明时的默认形态（预览第 ② 项在「没人引用它」时的答案）。
///
/// 只列**能通过写入门**的形态：新技能未受信任，而未受信任 + `full` 会被
/// [`agentpipeline_core::config::parse_skill_decls`] 拒绝，故默认只能是 `name`。
/// 要全文注入就必须先显式信任——这正是票面要的「默认不是已信任」。
const NEW_SKILL_DEFAULT_NOTE: &str = "该技能尚未被任何阶段或节点引用。新装技能默认为未受信任，\
     未受信任的声明只能以 name 模式写入（正文由 Skill 工具按需拉取）；要全文注入须先显式信任此技能";

/// 三项预览的统一组装（两个入口共用，避免装前 / 装后两份口径）。
///
/// `body_available` 为假时第 ③ 项是**空的**而非「无风险」：拿不到正文（装前预览的包里没有
/// `SKILL.md`，或磁盘上那份读不到）时界面必须显示「无正文可扫」，而不是「未发现特征」——
/// 后者是虚假的安心。
fn preview_json(
    name: &str,
    configs: &[StageConfig],
    scan: FeatureScan,
    body_available: bool,
) -> serde_json::Value {
    let recommendations: Vec<serde_json::Value> = skill_preview::recommendations_for(name)
        .into_iter()
        .map(|r| {
            json!({
                "stage": r.stage.as_str(),
                "reason": r.reason,
            })
        })
        .collect();

    let declarations: Vec<serde_json::Value> = skill_preview::declarations_for(configs, name)
        .into_iter()
        .map(|d| {
            json!({
                "declared_in": d.declared_in,
                "mode": d.mode,
                "trusted": d.trusted,
                // 裸字符串是信任概念出现前的老写法，界面要按 full + 未信任解释并提示
                "bare": d.bare,
            })
        })
        .collect();

    let has_declarations = !declarations.is_empty();
    let mut counts = serde_json::Map::new();
    for kind in [
        FeatureKind::RunCommand,
        FeatureKind::Network,
        FeatureKind::Credentials,
    ] {
        counts.insert(kind.as_str().to_string(), json!(scan.count(kind)));
    }
    let hits: Vec<serde_json::Value> = scan
        .hits
        .iter()
        .map(|h| {
            json!({
                "kind": h.kind.as_str(),
                "label": h.kind.label(),
                "line": h.line,
                "text": h.text,
            })
        })
        .collect();

    json!({
        "name": name,
        "recommendations": recommendations,
        "declarations": declarations,
        "defaults": {
            "mode": "name",
            "trusted": false,
            "note": if has_declarations { serde_json::Value::Null } else { json!(NEW_SKILL_DEFAULT_NOTE) },
        },
        "body_available": body_available,
        "features": {
            "hits": hits,
            "counts": serde_json::Value::Object(counts),
        },
    })
}

/// `GET /skills/{name}/preview`：已安装技能的三项预览。
///
/// 技能不在技能根里 → 404：装前预览走 `POST /skills/preview`（那个入口能拿到包内容），
/// 这里对着一个不存在的东西返回空三项只会让界面显示一堆「无」，比报错更误导。
pub async fn preview(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let root = state.home.skills_dir();
    if !discover(&root).iter().any(|s| s.name == name) {
        return Err(ApiError::not_found(format!(
            "技能 {name} 不在技能根 {} 下；装前预览请用 POST /skills/preview（带包体）",
            root.display()
        )));
    }
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;
    let body_available = root.join(&name).join(SKILL_FILE).is_file();
    let scan = skill_preview::scan_skill_body(&root, &name);
    Ok(Json(preview_json(&name, &configs, scan, body_available)))
}

/// `POST /skills/preview?name=`：**装前**预览——包还没落盘，先看三项。
///
/// 与 `GET /skills/{name}/preview` 的差别只有一处：第 ③ 项扫的是**包里的字节**而不是磁盘上
/// 的文件。这一项恰恰是装前最该看的——落盘之后再说「它正文里有 curl」就晚了半步。
pub async fn preview_package(
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
    let info = package.validate().map_err(map_core_error)?;
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;

    // 包内 `SKILL.md`：优先 `{name}/SKILL.md`，平铺打包的取根下的那份
    let raw = package
        .files
        .get(&format!("{}/{}", package.name, SKILL_FILE))
        .or_else(|| package.files.get(SKILL_FILE));
    let scan = raw
        .map(|bytes| skill_preview::scan_body(&String::from_utf8_lossy(bytes)))
        .unwrap_or_default();

    let mut out = preview_json(&info.name, &configs, scan, raw.is_some());
    out["install"] = json!({
        "name": info.name,
        "description": info.description,
        "sibling_count": info.sibling_count,
    });
    Ok(Json(out))
}

// ────────────────── PUT /skills/{name}/trust（票 11）──────────────────

#[derive(Debug, Deserialize)]
pub struct TrustBody {
    /// `true` = 显式信任；`false` = 撤销信任。
    pub trusted: bool,
}

/// `PUT /skills/{name}/trust`：把**引用该技能的每一条声明**改成给定的信任态。
///
/// 信任态不另存一份账（见模块头注释），故这个动作必然落在配置上：它改写
/// `skills_json` 与 `node_overrides_json[node].skills` 里引用该技能的条目，然后走
/// **与 `PUT /stage-configs` 同一道校验门**再落盘——校验不过就一条都不写。
///
/// 撤销信任撞上 `full` 声明时**拒绝**而不是静默降级（[`agentpipeline_core::config::set_skill_trust`]
/// 的纪律）：静默把 `full` 改成 `name` 会悄悄停掉一个正在生效的知识源。
pub async fn set_trust(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<TrustBody>,
) -> ApiResult<impl IntoResponse> {
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;

    let now = state.store.now();
    let mut updated: Vec<StageConfig> = Vec::new();
    for cfg in &configs {
        // 这里的 `Error::Config` 由**请求要求的这次改写**触发（降信任撞上 full 声明），
        // 用户按提示改掉 mode 即可，故与 `validate_prospective` 同判为 400：走通用映射会
        // 落到 500「配置错误」，把一条能自己修的问题报成服务器故障。
        if let Some(mut next) =
            agentpipeline_core::config::set_skill_trust(cfg, &name, body.trusted)
                .map_err(|e| ApiError::bad_request(e.to_string()))?
        {
            next.updated_at = now;
            updated.push(next);
        }
    }

    // 现有配置 + 改写后的那些 → 整体校验（同 `PUT /stage-configs` 的准入语义）
    let mut prospective: Vec<StageConfig> = configs
        .iter()
        .filter(|c| !updated.iter().any(|u| u.stage == c.stage))
        .cloned()
        .collect();
    prospective.extend(updated.iter().cloned());
    crate::routes::stage_configs::validate_prospective(&state, prospective, None).await?;

    for cfg in &updated {
        state
            .store
            .upsert_stage_config(cfg)
            .await
            .map_err(map_core_error)?;
    }

    let stages: Vec<String> = updated.iter().map(|c| c.stage.clone()).collect();
    Ok(Json(json!({
        "name": name,
        "trusted": body.trusted,
        "changed": stages.len(),
        "updated_stages": stages,
        "note": if stages.is_empty() {
            json!("该技能尚未被任何阶段或节点引用，配置未改动；请在阶段配置里启用它之后再确认信任态")
        } else {
            serde_json::Value::Null
        },
    })))
}

// ───────────────── 阶段推荐与一键安装（决策 172①，票 16）─────────────────

/// `GET /skills/recommendations`：按阶段列出推荐技能。
///
/// 推荐清单的**投递载体是界面而不是二进制**（决策 172①）：清单本身是代码内常量
/// （[`agentpipeline_core::agent::skill_preview::STAGE_RECOMMENDATIONS`]），技能正文一律
/// 由用户自己安装。这里把常量翻成界面能直接用的形态，并附上两件界面必须知道的事实：
/// **装没装**（未装显示「未安装」而不是崩掉）与**被谁引用**。
pub async fn recommendations(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let root = state.home.skills_dir();
    let installed: Vec<String> = discover(&root).into_iter().map(|s| s.name).collect();
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;

    let stages: Vec<serde_json::Value> = agentpipeline_core::types::ALL_STAGES
        .into_iter()
        .filter_map(|stage| {
            let names = skill_preview::recommended_skill_names(stage);
            if names.is_empty() {
                return None;
            }
            let items: Vec<serde_json::Value> = names
                .into_iter()
                .map(|name| {
                    let reason = skill_preview::recommendations_for(name)
                        .into_iter()
                        .find(|r| r.stage == stage)
                        .map(|r| r.reason)
                        .unwrap_or_default();
                    // 定位字段（决策 194）：清单的全部意义是"给还没装的人照着装"，
                    // 而 GitHub 模式下"照着装"要知道哪个仓的哪个目录。
                    let located = skill_preview::STAGE_RECOMMENDATIONS
                        .iter()
                        .find(|rec| rec.stage == stage.as_str() && rec.name == name);
                    json!({
                        "name": name,
                        "reason": reason,
                        "installed": installed.iter().any(|n| n == name),
                        "declared_in": agentpipeline_core::config::declared_skill_where(
                            &configs, name,
                        ),
                        "repo": located.map(|rec| rec.repo),
                        "dir": located.map(|rec| rec.dir),
                    })
                })
                .collect();
            Some(json!({ "stage": stage.as_str(), "skills": items }))
        })
        .collect();

    Ok(Json(json!({
        "stages": stages,
        "skills_root": root,
    })))
}

// ───────────────────────── POST /skills/install ─────────────────────────

#[derive(Debug, Deserialize)]
pub struct InstallRecommendedBody {
    /// 要写进哪个阶段的配置（**任意真实阶段**：推荐清单只是建议，用户可以把技能放进别处）。
    pub stage: String,
    /// 技能名（已在技能根里则不再下载；否则须在已放行来源的索引里存在）。
    pub name: String,
    /// 同名覆盖的**显式确认**（与票 09 / 10 同口径，默认拒绝）。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /skills/install`：一键安装 = 装技能到技能根 + 写进该阶段配置，一步完成。
///
/// ## 为什么必须经过票 11 的预览
///
/// 装完立刻写配置意味着**用户没有机会先看一眼正文**，而正文里可能有 `curl` / `.env` 这类
/// 特征。故本端点强制两件事：
///
/// 1. 写进去的声明**只能是 `name` 模式 + 未信任**——新装的技能还没被人确认过，
///    而未信任 + 全文注入会被票 05 的写入门拒绝（这里不绕过它，而是本来就写成合法形态）。
///    要全文注入，用户得回阶段配置界面显式点「信任」；
/// 2. 响应体带上完整的**三项预览**（推荐去向 / 注入模式与信任态 / 正文特征扫描），
///    让界面在装完之后立刻把特征命中摆给用户看。
///
/// ## 失败要可归因
///
/// 复用 [`crate::routes::market::map_market_error`]：技能不存在 → 404、来源未放行 → 400、
/// 摘要不符 → 400（带 `detail` 诊断）、网络失败 → 502。四种动作毫无交集，不能混成
/// 「安装失败」。
///
/// ## 已在技能根里的技能：跳过下载，只补配置那一步
///
/// 票 16 的界面约定是「未安装的项带安装按钮，**已安装的可直接启用**」：装与启用是两件事，
/// 而本端点把两者合成一步。若已在技能根里还去下载，票 09 的「同名不覆盖」会以 409 挡下，
/// 于是「已装但没在这个阶段启用」变成一条走不通的路（用户只剩手改配置一条）。
/// 故本地已有同名 markdown 技能且未显式 `overwrite` 时**只写配置**，响应里用 `note` 说明
/// 未重新下载——不是静默跳过：覆盖仍须 `overwrite` 显式确认，语义与票 09 / 10 一致。
pub async fn install_recommended(
    State(state): State<AppState>,
    Json(body): Json<InstallRecommendedBody>,
) -> ApiResult<impl IntoResponse> {
    if agentpipeline_core::types::Stage::from_str(&body.stage).is_err() {
        return Err(ApiError::bad_request(format!(
            "未知阶段：{}（推荐清单只覆盖十个真实阶段，伪阶段不跑 agent 节点）",
            body.stage
        )));
    }
    // 名字先过票 09 的同一条不变量：本地查找要把它拼进技能根路径，未校验的名字
    //（`../x` 之类）会让这次查找读到技能根之外。索引里的名字由 install_from_market 自查。
    let name = skill_import::check_skill_name(&body.name).map_err(|reason| {
        ApiError::bad_request(format!("技能名不能用作目录名（{reason}）：{}", body.name))
    })?;
    let root = state.home.skills_dir();
    // 清单里这条推荐的**定位**（决策 194）：仓 + 目录。取不到说明这个名字不在清单里
    // ——那不是错误（用户可以把任意技能放进任意阶段），只是没有可定位的来源。
    let located = skill_preview::STAGE_RECOMMENDATIONS
        .iter()
        .find(|rec| rec.stage == body.stage && rec.name == name)
        .copied();

    let already_installed = local_skill_package(&root, &name);
    let recorded = state
        .store
        .skill_source(&name)
        .await
        .map_err(map_core_error)?;

    // 「已装的那一份**就是**清单指的这份吗」——三种答案，对应三种动作。
    //
    // 判据按 `(owner/repo, 子路径)`，**不比 commit**：清单是一个**指针**（仓 + 目录），
    // 不是一份带版本的名录。它若钉死 commit，那份常量会随上游漂移变成陈旧数据；而"和当前
    // tip 比"又要为此多发一次网络请求——那正好毁掉这一条分支存在的理由（决策 181⑦：
    // 未配来源时也能走通，因为一次网络请求都不发生）。跳过时**原样保留**记录里的 commit，
    // 所以两种沉默都不会发生：既不静默换旧版（没换），也不静默升级（没升）。
    //
    // **「没有记录」算一致**（手工拷进来 / 本地导入 / 扫描进来的技能都没有记录）：那是
    // 决策 181⑦ 原本就要保住的那条路——「已装但没在这个阶段启用」必须走得通，且不许因此
    // 变成一次网络请求。票 02 的字面（「不一致则不跳过」）针对的是**有记录却指着别处**那种
    // 真歧义：那一份确知来自别的仓，就不能假装它是清单这份，要让它撞同名冲突由用户裁决。
    let installed_is_the_listed_one = match (already_installed.as_ref(), located) {
        // 不在清单里：没有「清单那份」可言，已装的就是用户要启用的那份
        (Some(_), None) => true,
        (Some(_), Some(loc)) => match recorded.as_ref() {
            Some(rec) => rec.matches_slug_and_path(loc.repo, loc.dir),
            None => true,
        },
        (None, _) => false,
    };

    let (info, note) = match (
        body.overwrite,
        already_installed,
        installed_is_the_listed_one,
    ) {
        (false, Some(info), true) => (
            info,
            Some(
                "技能已在技能根里，且就是要启用的那一份——本次未重新下载，只写配置。\
                 要换成清单指向的另一份请带 overwrite 显式确认",
            ),
        ),
        // 其余三种都走安装：未装（要下载）、显式覆盖（要重装）、已装但**确知**来自别处
        // （不跳过，让它撞同名冲突，由用户显式裁决——不静默换成旧版，也不静默升级）。
        _ => install_from_recommendation(&state, &root, &name, located, body.overwrite).await?,
    };

    // 写进该阶段配置：只增不减（既有声明逐字保留），新条目只能是 name + 未信任
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;
    let existing = configs.iter().find(|c| c.stage == body.stage).cloned();
    let mut candidate = existing.clone().unwrap_or_else(|| StageConfig {
        stage: body.stage.clone(),
        ..Default::default()
    });
    candidate.skills_json = Some(append_skill_decl(
        candidate.skills_json.as_ref(),
        &info.name,
        &body.stage,
    )?);
    candidate.updated_at = state.store.now();

    let prospective = configs_with(&configs, &candidate);
    crate::routes::stage_configs::validate_prospective(&state, prospective, None).await?;
    state
        .store
        .upsert_stage_config(&candidate)
        .await
        .map_err(map_core_error)?;

    let body_available = root.join(&info.name).join(SKILL_FILE).is_file();
    let scan = skill_preview::scan_skill_body(&root, &info.name);
    Ok(Json(json!({
        "skill": {
            "name": info.name,
            "description": info.description,
            "sibling_count": info.sibling_count,
        },
        "note": note,
        "stage_config": candidate,
        "preview": preview_json(&info.name, &configs_with(&configs, &candidate), scan, body_available),
    })))
}

/// 按清单的定位（仓 + 目录）取**当前 tip**，再走既有的落盘入口。
///
/// 为什么这里取 tip 而不是取某个固定 commit：清单只给"哪一份技能"（仓 + 目录），
/// 而安装必须落到一个具体 commit 上——`head()` 就是把它定下来的那一步（只走握手，不下 pack）。
/// 装完之后记录里存的是**这个具体 commit**，于是"我装的是哪一份"从此是确定的。
async fn install_from_recommendation(
    state: &AppState,
    root: &std::path::Path,
    name: &str,
    located: Option<skill_preview::StageRecommendation>,
    overwrite: bool,
) -> ApiResult<(PackageInfo, Option<&'static str>)> {
    let Some(loc) = located else {
        // **404 + `skill_not_found`**，不是 400：这个端点要回答的是"这个技能能不能装"，
        // 答案与"这个仓里没有那个目录"是同一句话（用户动作同为**换技能**）。决策 194 裁决⑦
        // 把八类映射同批搬到这个端点上（票 02 明写：不搬就会变成"八类里有两类永远映射不到、
        // 界面按四类分支"），故这里复用同一个 `kind`，而不是自造第五种。
        return Err(ApiError::not_found(format!(
            "技能 {name} 不在推荐清单里，没有可定位的来源仓：请到「设置 · 技能市场」页\
             从仓列表里安装它，或在配置里声明一个已装好的同名技能"
        ))
        .with_kind(agentpipeline_core::agent::repo::KIND_SKILL_NOT_FOUND));
    };
    let id = agentpipeline_core::agent::repo::RepoId::parse(loc.repo)
        .map_err(|e| ApiError::internal(format!("推荐清单里的仓名不合法（{}）：{e}", loc.repo)))?;
    // 放行判定早于任何网络动作（唯一的安全控制，与 /market/install 同一个函数）。
    agentpipeline_core::agent::repo::repo_allowed(&id, &state.market_repos())
        .map_err(crate::routes::market::map_market_error)?;

    let commit = state
        .repo()
        .head(&id)
        .await
        .map_err(crate::routes::market::map_market_error)?;
    let package = state
        .repo()
        .read_skill(&id, &commit, loc.dir)
        .await
        .map_err(crate::routes::market::map_market_error)?;
    // 冲突报文要与 `/market/install` 同口径：引擎只知道技能根下的路径，而"将要被覆盖的
    // 是哪一份"要报的是**仓坐标**（决策 194）。两条路共用同一个替换函数。
    let info = match skill_import::install(root, &package, overwrite) {
        Ok(info) => info,
        Err(err) => {
            return Err(
                crate::routes::market::conflict_with_origin(state, &package.name, err).await,
            )
        }
    };

    state
        .store
        .record_skill_source(&agentpipeline_core::storage::SkillSource {
            name: info.name.clone(),
            owner: id.owner.clone(),
            repo: id.name.clone(),
            commit_sha: commit.as_str().to_string(),
            subpath: loc.dir.to_string(),
            installed_at: state.store.now().to_rfc3339(),
        })
        .await
        .map_err(map_core_error)?;

    Ok((info, None))
}

/// 技能根里已存在的同名 markdown 技能的包元数据（`{name}/SKILL.md`）。
///
/// 只认技能根下的目录：技能的正文必须真的存在（不存在就没什么可顶替的）。名字由调用方先过
/// [`skill_import::check_skill_name`]，故这里的 `join` 不会走出技能根。
fn local_skill_package(root: &std::path::Path, name: &str) -> Option<PackageInfo> {
    let dir = root.join(name);
    if !dir.join(SKILL_FILE).is_file() {
        return None;
    }
    SkillPackage::from_dir(&dir).ok()?.validate().ok()
}

/// 把新装的技能追加进一份声明数组：**已声明就不重复追加**（只增不减，但也不重复）。
///
/// 既有元素逐字保留（含老格式的裸字符串与一时解析不通的杂值），与
/// [`agentpipeline_core::config::set_skill_trust`] 的改写姿态一致。
fn append_skill_decl(
    value: Option<&serde_json::Value>,
    name: &str,
    stage: &str,
) -> ApiResult<serde_json::Value> {
    let mut items: Vec<serde_json::Value> = match value {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => a.clone(),
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "阶段 {stage} 的 skills_json 不是数组：{other}；请先在阶段配置里修正它"
            )))
        }
    };
    let already = items.iter().any(|item| match item {
        serde_json::Value::String(s) => s == name,
        serde_json::Value::Object(o) => o.get("name").and_then(|v| v.as_str()) == Some(name),
        _ => false,
    });
    if !already {
        // 新装技能 = 未受信任 → 只能写 name 模式（写 full 会被票 05 的门拒绝）
        items.push(json!({ "name": name, "mode": "name", "trusted": false }));
    }
    Ok(serde_json::Value::Array(items))
}

/// 把刚写下的那份配置替换进集合，供预览的「注入模式与信任态」读到最新值。
fn configs_with(configs: &[StageConfig], candidate: &StageConfig) -> Vec<StageConfig> {
    let mut out: Vec<StageConfig> = configs
        .iter()
        .filter(|c| c.stage != candidate.stage)
        .cloned()
        .collect();
    out.push(candidate.clone());
    out
}
