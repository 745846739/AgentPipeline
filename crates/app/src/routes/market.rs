//! 技能来源：**仓名单**、列技能、按钉住的 commit 安装（决策 194）。
//!
//! | 端点 | 作用 |
//! |---|---|
//! | `GET /market/repos` | 当前的仓名单、它来自哪一级（界面 / 配置文件）、内置的冷启动推荐名单 |
//! | `PUT /market/repos` | 保存仓名单（校验 → 落库 → **当场生效**，不必重启） |
//! | `DELETE /market/repos` | 清掉界面那份，回到 `config.toml` 的 `[market] github_repos` |
//! | `GET /market/skills?repo=&q=&refresh=` | 列出一个仓里的技能，钉住那一刻的 commit |
//! | `POST /market/install` | 按**列表上那个** commit 装一个技能（复用票 09 的落盘入口） |
//!
//! ## 与旧层的关系（决策 194）
//!
//! 旧的三组端点（`GET /market/search`、`POST /market/install`（按名字）、`GET|PUT|DELETE /market/config`）
//! 随「自定 `/index.json` registry 整层退场」一起退场。**判定的对象变了**：从来源 origin 换成
//! `owner/repo`——GitHub 模式下字节的来源恒为 `github.com`，按 origin 放行等于放行全世界任何
//! 作者的任何仓。判定本身只有一处实现（[`agentpipeline_core::agent::repo::repo_allowed`]）。
//!
//! ## 与票 09 的关系：市场是**另一个来源**，不是另一条落盘路径
//!
//! 读出来的技能目录在内存里重打成单根 zip，走同一个 `SkillPackage::from_zip` + `install`，
//! 因此结构校验、同名冲突、路径穿越防护一处生效、两处受益（远程包不许比本地上传的包享有更宽的路）。
//!
//! ## 仓名单能在运行时改（决策 187 的两级结构，决策 194 继承）
//!
//! 保存走 [`Store::set_market_repos_override`]，**保存完立刻生效**（端点每次读
//! [`AppState::market_repos`]，没有"缓存的客户端"要重搭）。优先级「界面 > 配置文件」，
//! 清掉界面那份就回到配置文件（`DELETE` 给的就是这条路）。
//! 两处口径共用同一个校验函数（[`agentpipeline_core::config::validate_market_repos`]）：
//! 放行一个仓等于允许从它下载引导 agent 的正文，这条判定不能有第二个版本。
//!
//! ## 列表为什么要"钉住"
//!
//! 用户在列表里看到的是某一份，装到的就必须是那一份——这是"看到的 = 装到的"唯一落点。
//! 故列表把当刻的 commit 记在 [`Listings`] 里，后续请求**复用同一个 commit**，直到用户显式
//! 刷新（`refresh=1` 重新 `head()`）。不这么做的话，锚会退化成"安装那一刻的 HEAD"，
//! 也就是本 effort 要消灭的那个移动靶。

use agentpipeline_core::agent::repo::{self, Oid, RepoId};
use agentpipeline_core::storage::SkillSource;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::{ApiError, ApiResult, AppState};

/// 内置的**冷启动推荐名单**（决策 194）。
///
/// 这一页是"我订阅了哪些仓 + 它们里有什么 + 装哪一个"，而仓级白名单把浏览面收在"我加过的仓"上
/// ——于是冷启动时这一页是**空的**：用户得凭空知道一个仓名，一个叫"浏览发现"的页在冷启动时
/// 是空白输入框，等于把这一档的意义抹掉一半。（实测结论是生态里没有聚合目录：
/// 71 个主机 0 家发布我们的索引。）
///
/// **内置 ≠ 放行。** 这些只是若干条**用户可以删的配置默认值**，不是一份审核过的目录：
/// 不在名单里点"添加"之前**一个字节都不下载**（不 fetch、不 head）。这一条守住，
/// 仓级白名单的代价曲线才不被这次的便利性侵蚀。
///
/// 清单就是本 effort 实测过的那些公开技能仓。
pub const RECOMMENDED_REPOS: &[&str] = &[
    "obra/superpowers",
    "mattpocock/skills",
    "anthropics/skills",
    "vercel-labs/agent-skills",
    "wshobson/agents",
    "pulumi/agent-skills",
];

/// 列表钉住的那一份：`(commit, 取到它的时刻)`。
///
/// 进程内、按仓、不过期：它的全部意义就是"两次列表之间不漂移"，而用户点一次刷新就该换掉它。
/// 不做持久化——重启之后重新 `head()` 是对的（那时也没有"用户正看着的那一份"了）。
pub type Listings = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Listing>>>;

/// 一份列表读数。
#[derive(Debug, Clone)]
pub struct Listing {
    pub commit: String,
    /// RFC3339（界面显示"基于 `<短 SHA>`（时间）"）。
    pub listed_at: String,
}

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/market/repos",
            get(repos).put(save_repos).delete(clear_repos),
        )
        .route("/market/skills", get(skills))
        .route("/market/install", post(install))
        .with_state(state)
}

// ─────────────────────── GET / PUT / DELETE /market/repos ───────────────────────

/// `GET /market/repos`：界面上的仓名单编辑器要的读数。
///
/// `origin` 说清这一份**是谁定的**（界面 / 配置文件）——用户改 `config.toml` 却发现
/// 「改了没用」时，答案必须在这一页上看得见，而不是靠他去猜优先级。
pub async fn repos(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    Ok(Json(repos_json(&state)))
}

fn repos_json(state: &AppState) -> serde_json::Value {
    let from_settings = state.market_override().is_some();
    json!({
        "repos": state.market_repos(),
        "origin": if from_settings { "settings" } else { "config" },
        // 冷启动推荐：只是字符串，界面不点"添加"就不会有任何网络请求（见 `RECOMMENDED_REPOS`）
        "recommended": RECOMMENDED_REPOS,
    })
}

#[derive(Debug, Deserialize)]
pub struct ReposBody {
    /// 归一前的仓名列表（含粘贴残留照收，校验函数会归一）。
    #[serde(default)]
    pub repos: Vec<String>,
}

/// `PUT /market/repos`：保存界面上的仓名单（决策 187 的两级结构，决策 194 继承）。
///
/// 两步，顺序是刻意的：**先校验 → 再落库**。落库失败就整条失败，内存里的那一份不动
/// （「界面上显示改了、重启后却不是」最坏）。这一步没有"第三步换客户端"了：GitHub 模式下
/// 访问层与来源无关（URL 由 `owner/repo` 拼），端点每次读的都是仓名单本身，所以保存**天然即生效**。
///
/// 保存是**显式动作**：空数组是合法输入（= 不从任何仓安装），与「没保存过」不同。
pub async fn save_repos(
    State(state): State<AppState>,
    Json(body): Json<ReposBody>,
) -> ApiResult<impl IntoResponse> {
    let repos = agentpipeline_core::config::validate_market_repos(&body.repos)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    state
        .store
        .set_market_repos_override(&repos)
        .await
        .map_err(crate::state::map_core_error)?;
    state.set_market_override(repos);
    Ok(Json(repos_json(&state)))
}

/// `DELETE /market/repos`：清掉界面那份，回到 `config.toml` 的 `[market] github_repos`。
pub async fn clear_repos(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    state
        .store
        .clear_market_repos_override()
        .await
        .map_err(crate::state::map_core_error)?;
    state.clear_market_override();
    Ok(Json(repos_json(&state)))
}

// ──────────────────────── GET /market/skills ────────────────────────

#[derive(Debug, Deserialize)]
pub struct SkillsQuery {
    /// 仓名 `owner/repo`（须在生效的仓名单里）。
    pub repo: String,
    /// 关键词（名字或描述命中；省略 = 列出全部）。
    #[serde(default)]
    pub q: Option<String>,
    /// 重新取分支 tip（省略 = 复用这一份已钉住的 commit）。
    #[serde(default)]
    pub refresh: Option<String>,
}

/// `GET /market/skills`：列出一个仓里的技能，**钉住**当刻的 commit。
///
/// 顺序：**先判放行**（未放行的仓不进列表——看不到装不上的东西，与安装侧同口径），
/// 再取 commit（`refresh` 或不缓存时 `head()`，否则复用），再列技能。
///
/// 列技能在本地对象库上做（`read_skill` 那一层不联网），故关键词过滤是**本地过滤**，
/// 不引 GitHub search API（那个接口的配额是 10 次/小时，且决策 194 明确不引 API 面）。
/// 跨仓搜索因此只覆盖已 fetch 过的仓——这是有意的取舍，界面不该假装它能搜全。
pub async fn skills(
    State(state): State<AppState>,
    Query(query): Query<SkillsQuery>,
) -> ApiResult<impl IntoResponse> {
    let id = RepoId::parse(&query.repo).map_err(map_market_error)?;
    let slug = id.slug();
    agentpipeline_core::agent::repo::repo_allowed(&id, &state.market_repos())
        .map_err(map_market_error)?;

    let commit = listing_commit(&state, &id, want_refresh(query.refresh.as_deref())).await?;

    let entries = state
        .repo()
        .list_skills(&id, &commit.0)
        .await
        .map_err(map_market_error)?;

    let needle = query.q.unwrap_or_default().trim().to_lowercase();
    let hits: Vec<&repo::SkillRef> = entries
        .iter()
        .filter(|s| {
            needle.is_empty()
                || s.name.to_lowercase().contains(&needle)
                || s.description
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&needle))
        })
        .collect();

    // 按技能目录的父路径分组，不摊平：`wshobson/agents` 是 183 个技能 / 94 个插件，
    // 摊平了没法看；而父路径是**扫描时免费得到的**，不需要读任何索引文件。
    let mut groups: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    for hit in hits {
        let parent = hit.dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let item = json!({
            "name": hit.name,
            "dir": hit.dir,
            "description": hit.description,
        });
        match groups.iter_mut().find(|(path, _)| path == parent) {
            Some((_, items)) => items.push(item),
            None => groups.push((parent.to_string(), vec![item])),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(Json(json!({
        "repo": slug,
        "commit": commit.0.as_str(),
        "commit_short": commit.0.short(),
        "listed_at": commit.1,
        "groups": groups
            .into_iter()
            .map(|(path, skills)| json!({ "path": path, "skills": skills }))
            .collect::<Vec<_>>(),
    })))
}

/// `refresh` 取法：真值只有 `1` / `true`（大小写不敏感）两种写法。
///
/// 不做"任何非空字符串都算刷新"这种宽松解析：一个拼错的值（`refresh=yes`）静默变成
/// "不刷新"会让用户以为刷新没生效，而那次点击本该是显式动作。
fn want_refresh(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1") | Some("true")
    )
}

/// 取这一份列表要用的 commit + 它的取到时刻。
///
/// ## `refresh` 的语义是"我按了刷新钮"（决策 194）
///
/// **只在用户显式刷新时重新 `head()`**：不刷新就复用缓存里那个 commit。这是"列表钉住浏览
/// 那一刻的 commit"的全部落点——若每次列表都重新 `head()`，用户看到的 SHA 会在他眼皮底下变，
/// 而"看到的 = 装到的"就不再成立。
///
/// `refresh` 但取不到新 tip（网络失败）时**报错而不是悄悄用旧的**：用户按了刷新、界面显示
/// 了一个旧 SHA 却没有任何提示，是比失败更坏的结果（他会拿旧的那份去安装）。
async fn listing_commit(state: &AppState, id: &RepoId, refresh: bool) -> ApiResult<(Oid, String)> {
    let slug = id.slug();
    if !refresh {
        let cached = state
            .listings
            .lock()
            .ok()
            .and_then(|guard| guard.get(&slug).cloned());
        if let Some(hit) = cached {
            let commit = Oid::parse(&hit.commit).map_err(map_market_error)?;
            return Ok((commit, hit.listed_at));
        }
    }
    // 缓存未命中 / 显式刷新：`head()` 只走握手，不下载 pack。
    let commit = state.repo().head(id).await.map_err(map_market_error)?;
    let listed_at = chrono::Utc::now().to_rfc3339();
    if let Ok(mut guard) = state.listings.lock() {
        guard.insert(
            slug,
            Listing {
                commit: commit.as_str().to_string(),
                listed_at: listed_at.clone(),
            },
        );
    }
    Ok((commit, listed_at))
}

// ────────────────────────── POST /market/install ──────────────────────────

#[derive(Debug, Deserialize)]
pub struct InstallBody {
    pub owner: String,
    pub repo: String,
    /// **完整 40 位** commit SHA——用户在列表里看到的是哪一份，这里就必须是哪一份。
    /// 界面负责原样回传，后端**不解析 HEAD**（否则锚退化成"安装那一刻的 tip"）。
    pub commit: String,
    /// 技能目录在仓根内的相对路径（仓根技能是空串）。
    #[serde(default)]
    pub subpath: String,
    /// 同名覆盖的**显式确认**（与票 09 同口径，默认拒绝）。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /market/install`：按钉住的 commit 读技能 → 既有的落盘入口 → 记一行来源。
///
/// 三步各有其不可省的理由：
/// 1. **判放行**（`repo_allowed`）——唯一的安全控制，早于任何网络动作；
/// 2. `read_skill` → 内存 zip 往返 → `from_zip` → `install`——落盘侧零改动；
/// 3. **写来源记录**——同名冲突要说清"装的是哪个仓哪个版本"，而仓名 / commit / 子路径
///    一样都不在落盘路径里，只能记（见迁移 0011 的注释）。
pub async fn install(
    State(state): State<AppState>,
    Json(body): Json<InstallBody>,
) -> ApiResult<impl IntoResponse> {
    let id = RepoId::parse(&format!("{}/{}", body.owner, body.repo)).map_err(map_market_error)?;
    let commit = Oid::parse(&body.commit).map_err(map_market_error)?;
    agentpipeline_core::agent::repo::repo_allowed(&id, &state.market_repos())
        .map_err(map_market_error)?;

    let package = state
        .repo()
        .read_skill(&id, &commit, &body.subpath)
        .await
        .map_err(map_market_error)?;
    let name = package.name.clone();
    let root = state.home.skills_dir();
    let info =
        match agentpipeline_core::agent::skill_import::install(&root, &package, body.overwrite) {
            Ok(info) => info,
            Err(err) => return Err(conflict_with_origin(&state, &name, err).await),
        };

    state
        .store
        .record_skill_source(&SkillSource {
            name: info.name.clone(),
            owner: id.owner.clone(),
            repo: id.name.clone(),
            commit_sha: commit.as_str().to_string(),
            subpath: body.subpath.clone(),
            installed_at: state.store.now().to_rfc3339(),
        })
        .await
        .map_err(crate::state::map_core_error)?;

    Ok(Json(json!({
        "skill": {
            "name": info.name,
            "description": info.description,
            "sibling_count": info.sibling_count,
        },
        "source": format!("{}@{}", id.slug(), commit.short()),
    })))
}

// ─────────────────────────── 错误映射与报文 ───────────────────────────

/// 八类失败 → HTTP 状态码 + `kind`。
///
/// | `kind` | 状态码 | 用户要做的动作 |
/// |---|---|---|
/// | `market_network` | 502 | 重试（下游不可达） |
/// | `repo_not_found` / `commit_not_found` / `skill_not_found` / `repo_unreadable` | 404 | 改仓名 / 换 commit / 换技能 / 换仓 |
/// | `repo_not_allowed` / `digest_mismatch` / `download_too_large` | 400 | 去加白名单 / 别装报警 / 换更小的仓 |
///
/// **判据是"每类对应一个互不相同的用户动作"，不是状态码好看**。故状态码可以撞（三个 404
/// 挤在一起是有意的：它们对 HTTP 客户端而言都是"你要的东西不在那儿"），但 `kind` 必须分得开
/// ——界面按 `kind` 分支，不按状态码、更不按报文里的字样。
///
/// `Error::Market` 的 `raw`（期望与实际、HTTP 状态、目录名）经 `with_detail` 作为响应体的
/// `detail` 字段下发，与面向用户的 `message` 分开。
pub(crate) fn map_market_error(err: agentpipeline_core::Error) -> ApiError {
    use agentpipeline_core::agent::repo as r;
    let detail = err.market_kind().map(|(_, raw)| raw.to_string());
    let kind = err.market_kind().map(|(kind, _)| kind.to_string());
    let message = err.to_string();
    let mapped = match kind.as_deref() {
        Some(r::KIND_NETWORK) => ApiError::bad_gateway(message),
        Some(r::KIND_REPO_NOT_FOUND)
        | Some(r::KIND_COMMIT_NOT_FOUND)
        | Some(r::KIND_SKILL_NOT_FOUND)
        | Some(r::KIND_REPO_UNREADABLE) => ApiError::not_found(message),
        Some(_) => ApiError::bad_request(message),
        // 同名冲突（票 09 的 `Conflict`）→ 409，其余走通用映射
        None => crate::state::map_core_error(err),
    };
    let mapped = match detail {
        Some(d) => mapped.with_detail(d),
        None => mapped,
    };
    match kind {
        Some(k) => mapped.with_kind(k),
        None => mapped,
    }
}

/// 同名冲突的报文要把「当前来源」换成**来源记录里的仓坐标**（决策 194）。
///
/// ## 为什么在路由层换，而不是改引擎
///
/// 引擎（`skill_import::install`）只知道技能根下的路径，而 GitHub 模式下仓名 / commit /
/// 子路径**一样都不在路径里**。票 01 / 02 的验收明写「引擎零改动可核对」，所以替换在这里做。
///
/// 代价是这个函数依赖引擎的报文模板，故 `crates/app/tests/market.rs` 有一条用例**真的调一次
/// `install`**、按模板断言那句报文——引擎哪天改了措辞，那条会红，而不是让这句替换静默失效。
///
/// **没有记录时原样返回**（手工拷进来、本地导入、扫描进来的技能都没有记录）：
/// 那时引擎那句「技能根下的 `<路径>`」就是正确答案，不能因为查不到来源就报不出来源。
///
/// 两个安装入口共用它（`POST /market/install` 与一键安装 `POST /skills/install`）：
/// 「将要覆盖的是哪一份」在两条路上是同一个问题，判定与措辞不该各写一遍。
pub(crate) async fn conflict_with_origin(
    state: &AppState,
    name: &str,
    err: agentpipeline_core::Error,
) -> ApiError {
    let agentpipeline_core::Error::Conflict(msg) = &err else {
        return map_market_error(err);
    };
    // 查不到（含查询本身失败）一律当「没有记录」：报文少一句仓坐标，胜过因为一次查库失败
    // 就把冲突错误换成 500——那会让用户以为"装不上"而不是"重名了"。
    let recorded = state.store.skill_source(name).await.ok().flatten();
    match recorded {
        Some(source) => map_market_error(agentpipeline_core::Error::Conflict(
            replace_source_clause(msg, &source.describe()),
        )),
        None => map_market_error(err),
    }
}

/// 引擎报文里的「当前来源」那一栏的起点。
///
/// 与 `crates/app/tests/market.rs` 里那条"真的调一次 install"的用例配对：模板变了，
/// 那条红，这里也就一起被看见。
const SOURCE_CLAUSE_OPEN: &str = "（当前来源：";
const SOURCE_CLAUSE_CLOSE: &str = "）；覆盖需显式确认";

/// 把「（当前来源：…）」里的内容换成仓坐标。
///
/// 模板对不上时**原样返回**：宁可少说一句仓坐标，也不要拼出一句语法坏掉的话
/// （那比没有信息更糟——用户会以为系统坏了）。
fn replace_source_clause(msg: &str, origin: &str) -> String {
    let (Some(start), Some(end)) = (msg.find(SOURCE_CLAUSE_OPEN), msg.find(SOURCE_CLAUSE_CLOSE))
    else {
        return msg.to_string();
    };
    if end <= start {
        return msg.to_string();
    }
    let mut out = String::with_capacity(msg.len() + origin.len());
    out.push_str(&msg[..start + SOURCE_CLAUSE_OPEN.len()]);
    out.push_str(origin);
    out.push_str(&msg[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_flag_accepts_only_explicit_truthy_values() {
        assert!(want_refresh(Some("1")));
        assert!(want_refresh(Some("true")));
        assert!(want_refresh(Some(" TRUE ")));
        assert!(!want_refresh(Some("yes")));
        assert!(!want_refresh(Some("0")));
        assert!(!want_refresh(Some("")));
        assert!(!want_refresh(None));
    }

    /// 替换只动「当前来源」那一栏，前后两截逐字保留。
    #[test]
    fn source_clause_is_replaced_in_place() {
        let engine = "技能 grill 已存在（当前来源：技能根下的 /home/me/skills/grill/SKILL.md）；\
                      覆盖需显式确认";
        let out = replace_source_clause(engine, "obra/superpowers@553c207:skills/grill");
        assert_eq!(
            out,
            "技能 grill 已存在（当前来源：obra/superpowers@553c207:skills/grill）；\
             覆盖需显式确认"
        );
    }

    /// 模板对不上时原样返回——宁可少说一句，也不要拼出一句语法坏掉的话。
    #[test]
    fn source_clause_leaves_an_unrecognised_message_alone() {
        for msg in [
            "技能 grill 已存在；覆盖需显式确认",
            "技能 grill 已存在（当前来源：x）",
            "",
        ] {
            assert_eq!(replace_source_clause(msg, "a/b@1234567:c"), msg);
        }
    }

    #[test]
    fn recommended_repos_are_all_parseable_slugs() {
        // 内置清单是**配置默认值**，必须与用户手写的那份过同一个判定
        // （一个内置的非法值会让"添加"按钮永远点不动，而那是我们自己的 bug）
        for slug in RECOMMENDED_REPOS {
            RepoId::parse(slug).unwrap_or_else(|e| panic!("内置推荐名单里的 {slug} 不合法：{e}"));
        }
    }
}
