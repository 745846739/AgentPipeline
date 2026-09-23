//! L3 契约测试：技能来源（决策 194）。
//!
//! 与 `api_contract.rs` 的分工：那一位钉的是"其余端点在技能来源缺席时不受影响"，
//! 这一位钉的是**技能来源自己**。旧的那组（自定 `/index.json` registry + 注入 testkit
//! `FakeMarket`）随决策 194 整层退场，取而代之的是**更硬的一种离线**：
//!
//! - testkit 的 [`RepoFixture`] 起一个真的裸仓，[`SmartHttp`] 用系统 git 的 `upload-pack`
//!   把它按 smart HTTP 协议暴露出来（两条路由）；
//! - 后端注入**真的** [`Libgit2Repo`]，base 指回那个回环地址（与决策 177③ 同一条放行规则：
//!   回环放行明文 http）。
//!
//! 于是这组用例打到的是只有真 libgit2 才走得到的那几段：`depth(1)` 的 shallow fetch、
//! `RemoteRedirect::None`、流式字节上限的中断、按**裸 SHA** 取一个**非 tip** 的 commit、
//! 以及"对象真的落到本地对象库了吗"那道复验。替身打不到这些——而它们全是实测里踩过坑的地方。
//!
//! **默认门不打真网络**：fixture 全在本机回环。真 GitHub 的用例必须 opt-in，且不在这里。

use std::sync::Arc;

use agentpipeline_core::agent::repo::{Libgit2Repo, Oid, RepoId};
use agentpipeline_core::config::Settings;
use agentpipeline_core::storage::SkillSource;
use agentpipeline_core::types::Provider;
use app::{build_router, AppState};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use testkit::{RepoFixture, SmartHttp, TestHome};
use tower::ServiceExt;

const PORT: u16 = 8787;
/// fixture 的仓名那一半（`{owner}` 可以随便换，仓名固定这个——见 `RepoFixture::named`）。
const STEM: &str = "repo";
const OWNER: &str = "acme";
const SLUG: &str = "acme/repo";
/// 内置推荐清单里那些技能的仓（真实存在的仓名，一键安装的定位就指向它）。
const LISTED_SLUG: &str = "mattpocock/skills";

/// 一个技能目录的 `SKILL.md`（带 frontmatter，好让列表能读出 description）。
fn skill_md(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n")
}

/// 装置：临时 home + 真后端 router + 离线 git 仓（真 libgit2 打它）。
struct Api {
    _home: TestHome,
    /// 裸仓。用例要在"后端已经列过一次之后"再推进 tip，故它不是只读的。
    fixture: RepoFixture,
    /// 请求日志（`requests()`）与断言语境。没有 fixture 的用例是 `None`。
    http: Option<SmartHttp>,
    state: AppState,
    router: Router,
}

/// 最常见的一套：仓名 `repo`、两个技能（深度 2 与深度 4）、一个子树兄弟文件。
async fn api_with(repos: Vec<String>) -> Api {
    api_named(STEM, repos, |f| {
        f.add_file(
            "skills/grilling/SKILL.md",
            &skill_md("grilling", "拷问设计树", "正文"),
        )
        .unwrap();
        f.add_file("skills/grilling/refs/notes.md", "兄弟文件")
            .unwrap();
        f.add_file(
            "plugins/demo/skills/tdd/SKILL.md",
            &skill_md("tdd", "测试驱动", "红绿重构"),
        )
        .unwrap();
        // 不含 SKILL.md 的目录：不该出现在列表里
        f.add_file("docs/readme.md", "不是技能").unwrap();
        // 收尾匹配会误收的那种文件（实测 38 vs 37 的那一个）
        f.add_file("skills/.changeset/add-skill.md", "不是技能")
            .unwrap();
        f.commit("chore: 三个技能").unwrap();
    })
    .await
}

/// 起一套带 fixture 的装置。
async fn api_named<F>(stem: &str, repos: Vec<String>, build: F) -> Api
where
    F: FnOnce(&mut RepoFixture),
{
    let mut fixture = RepoFixture::named(stem).unwrap();
    build(&mut fixture);
    let http = SmartHttp::serve(fixture.dir()).await.unwrap();
    let (home, state) = base_state(repos, &http.base(), None).await;
    let router = build_router(state.clone());
    Api {
        _home: home,
        fixture,
        http: Some(http),
        state,
        router,
    }
}

/// 一套**没有真仓**的装置：仓访问层指向一个关闭的端口。
///
/// 用它来钉"这条路上一次网络请求都不该发生"——一旦真去取仓，用例会以 502 失败，
/// 而不是悄悄通过。
async fn api_offline(repos: Vec<String>) -> Api {
    let (home, state) = base_state(repos, "http://127.0.0.1:1", None).await;
    let router = build_router(state.clone());
    Api {
        _home: home,
        fixture: RepoFixture::named(STEM).unwrap(),
        http: None,
        state,
        router,
    }
}

/// 建 home + store + provider + `AppState`。
///
/// `TestHome` **必须跟着 `Api` 活**：它持有的 `TempDir` 一旦 drop，家目录就没了
/// （技能根、缓存库全在里面）。故它随返回值一起交出去，由 `Api::_home` 拿住。
async fn base_state(
    repos: Vec<String>,
    base: &str,
    max_bytes: Option<usize>,
) -> (TestHome, AppState) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    store
        .upsert_provider(&Provider {
            id: "p-default".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: Some("sk-test".into()),
            enabled: true,
            created_at: store.now(),
            updated_at: store.now(),
        })
        .await
        .unwrap();
    let mut repo = Libgit2Repo::new(cache_dir()).with_base(base).unwrap();
    if let Some(max) = max_bytes {
        repo = repo.with_max_bytes(max);
    }
    let state = AppState::new(store, home.home().clone(), Settings::default(), PORT)
        .with_allowed_origins(Vec::new())
        .with_repo(Arc::new(repo), repos);
    (home, state)
}

/// 每次调用一个独立的缓存目录（共用一个会让上一个用例取下的仓"看起来已经在了"）。
fn cache_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("agentpipeline-market-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ─────────────────────────────── 请求助手 ───────────────────────────────

async fn call(api: &Api, req: Request<Body>) -> (StatusCode, Value) {
    let response = api.router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn get(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, Request::get(uri).body(Body::empty()).unwrap()).await
}

async fn post(api: &Api, uri: &str, body: Value) -> (StatusCode, Value) {
    call(
        api,
        Request::post(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn put(api: &Api, uri: &str, body: Value) -> (StatusCode, Value) {
    call(
        api,
        Request::put(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn delete(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, Request::delete(uri).body(Body::empty()).unwrap()).await
}

fn skills_root(api: &Api) -> std::path::PathBuf {
    api.state.home.skills_dir()
}

/// 列表里所有技能的 (分组路径, 名字, 目录)。
fn listed(body: &Value) -> Vec<(String, String, String)> {
    body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| {
            let path = g["path"].as_str().unwrap().to_string();
            g["skills"].as_array().unwrap().iter().map(move |s| {
                (
                    path.clone(),
                    s["name"].as_str().unwrap().to_string(),
                    s["dir"].as_str().unwrap().to_string(),
                )
            })
        })
        .collect()
}

/// 列一次技能，返回 `(commit, 列表)`。
/// 列技能，要求 200。失败时把 **fixture 收到的请求行**一并报出来——这条路上的红多半是
/// 装置时序问题（客户端说 broken pipe、而服务端觉得自己什么都答了），没有这份日志就只能猜。
async fn list(api: &Api) -> (String, Vec<(String, String, String)>) {
    let (status, body) = get(
        api,
        &format!("/market/skills?repo={}", SLUG.replace('/', "%2F")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{body}｜fixture 收到：{:?}｜细读：{:?}",
        seen(api),
        trace_of(api)
    );
    (body["commit"].as_str().unwrap().to_string(), listed(&body))
}

/// fixture 迄今收到的请求行（没有 fixture 的装置是空的）。
fn seen(api: &Api) -> Vec<String> {
    api.http
        .as_ref()
        .map(|http| http.requests())
        .unwrap_or_default()
}

/// fixture 的细读（每趟请求的 header 与响应）。
fn trace_of(api: &Api) -> Vec<String> {
    api.http
        .as_ref()
        .map(|http| http.trace())
        .unwrap_or_default()
}

/// 按一个给定 commit 装一个技能。
async fn install(api: &Api, commit: &str, subpath: &str, overwrite: bool) -> (StatusCode, Value) {
    post(
        api,
        "/market/install",
        json!({"owner": OWNER, "repo": STEM, "commit": commit,
               "subpath": subpath, "overwrite": overwrite}),
    )
    .await
}

// ─────────────────────── 仓名单（两级结构，决策 187 / 194）───────────────────────

#[tokio::test]
async fn repos_config_defaults_to_the_config_level_with_no_repo() {
    let api = api_with(Vec::new()).await;
    let (status, body) = get(&api, "/market/repos").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["origin"], "config");
    assert_eq!(body["repos"].as_array().unwrap().len(), 0);
    // 冷启动名单是**字符串**，不是"已经放行的仓"——它就是"帮你起步"的那几条
    assert!(body["recommended"].as_array().unwrap().len() >= 5, "{body}");
}

/// 保存即生效（不重启）、清掉回到配置文件、**显式清空 ≠ 没保存过**。
#[tokio::test]
async fn repos_config_is_a_two_level_override() {
    let api = api_with(vec![SLUG.to_string()]).await;

    // ① 保存一份新的：当场生效（端点每次读的都是仓名单本身，没有"缓存的客户端"要重搭）
    let (status, body) = put(
        &api,
        "/market/repos",
        json!({"repos": ["Obra/Superpowers"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["origin"], "settings");
    assert_eq!(body["repos"][0], "Obra/Superpowers", "大小写照收不改");
    assert_eq!(
        api.state.market_repos(),
        vec!["Obra/Superpowers".to_string()]
    );

    // ② 显式清空 = 不从任何仓安装；仍在「界面」那一级（不是回到配置）
    let (status, body) = put(&api, "/market/repos", json!({"repos": []})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["origin"], "settings");
    assert_eq!(body["repos"].as_array().unwrap().len(), 0);
    assert!(
        api.state.market_override().is_some(),
        "空表是显式动作，不是没保存过"
    );

    // ③ DELETE 才是"回到 config.toml 那一级"
    let (status, body) = delete(&api, "/market/repos").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["origin"], "config");
    assert_eq!(api.state.market_repos(), vec![SLUG.to_string()]);
}

/// 非法仓名当场被拒，且报文说清是哪一项为什么。
#[tokio::test]
async fn repos_config_rejects_illegal_repo_names() {
    let api = api_with(Vec::new()).await;
    for (bad, needle) in [
        ("https://gitlab.com/a/b", "仓名不合法"),
        ("git@github.com:a/b.git", "仓名不合法"),
        ("a/../../etc", "仓名不合法"),
        ("just-a-name", "owner/repo"),
    ] {
        let (status, body) = put(&api, "/market/repos", json!({"repos": [bad]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {body}");
        assert!(
            body["error"].as_str().unwrap().contains(needle),
            "{bad} → {body}"
        );
    }
    // 拒绝之后界面那一级没被改动（"显示改了、实际没改"最坏）
    assert!(api.state.market_override().is_none());
}

/// 旧层**真的没了**：那三组老端点一律 404（不是 400、不是 410——路由根本没注册）。
///
/// 这条是票 04「整层退场」在契约层的落点。留一个半死的端点比删干净更坏：它会让「来源白名单」
/// 以旧语义（origin 口径）继续存在，而决策 194 裁决① 明写**不能并存**——并存会把「放行判定」
/// 与「失败分类」各变成两份实现，而放行判定是本系统唯一的安全控制。
#[tokio::test]
async fn the_retired_endpoints_are_gone() {
    let api = api_with(vec![SLUG.to_string()]).await;
    for (method, uri) in [
        ("GET", "/market/config"),
        ("PUT", "/market/config"),
        ("DELETE", "/market/config"),
        ("GET", "/market/search"),
        ("POST", "/market/index"),
        ("GET", "/market/index.json"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(&api, req).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {uri} 应当连同旧层一起退场了"
        );
    }
}

// ─────────────────────────── 列表：钉住 commit ───────────────────────────

/// 技能识别口径：含 `SKILL.md` 的目录就是技能、名字 = basename、**与深度无关**；
/// 按父路径分组；未放行的仓不进列表。
#[tokio::test]
async fn list_skills_scans_by_skill_md_and_groups_by_parent() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (status, body) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let rows = listed(&body);
    assert_eq!(
        rows.iter()
            .map(|(p, n, _)| (p.as_str(), n.as_str()))
            .collect::<Vec<_>>(),
        vec![("plugins/demo/skills", "tdd"), ("skills", "grilling")],
        "深度 2 与深度 4 都要认出来，且按父路径分组升序"
    );
    // 判据是 basename **精确**等于 `SKILL.md`：收尾匹配会把 `.changeset/add-skill.md` 误收
    assert!(
        !rows
            .iter()
            .any(|(_, n, _)| n == "add-skill" || n == "readme"),
        "{rows:?}"
    );
    assert_eq!(rows[1].2, "skills/grilling", "dir 是仓根相对路径");

    // 描述来自 frontmatter
    let grilling = body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["skills"].as_array().unwrap())
        .find(|s| s["name"] == "grilling")
        .unwrap();
    assert_eq!(grilling["description"], "拷问设计树");

    // 短 SHA 与完整 SHA 都在读数里（界面显示短的那个，安装回传完整的那个）
    assert_eq!(body["commit"].as_str().unwrap().len(), 40);
    assert_eq!(body["commit_short"].as_str().unwrap().len(), 7);
    assert!(body["listed_at"].as_str().unwrap().contains('T'));
}

/// `head()` **只握手，不下 pack**——请求日志里只该有 `info/refs`，一条 `git-upload-pack` 都不该有。
///
/// 这条是「列出一个仓有什么技能很便宜」的全部依据（票 01 的验收锚点）：`head` 走的是
/// `connect` + `list`（ls-remote），对象一个都不取。它若悄悄下载，列表页每次打开都要拖一个
/// pack——而界面上的「刷新」只是一颗按钮，用户以为它是免费的。
#[tokio::test]
async fn head_only_ls_remote_and_never_downloads_a_pack() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let id = RepoId::parse(SLUG).unwrap();
    let tip = api.state.repo().head(&id).await.expect("取 tip");
    assert_eq!(tip.as_str().len(), 40, "tip 要是完整 SHA：{tip}");

    let log = seen(&api);
    assert!(
        !log.is_empty(),
        "head 至少要打一次 info/refs——一条请求都没有说明它走了别的路"
    );
    assert!(
        log.iter().all(|line| line.contains("info/refs")),
        "head 阶段不该有别的请求：{log:?}"
    );
    assert!(
        !log.iter().any(|line| line.starts_with("POST")),
        "head 不得下载 pack：{log:?}"
    );
}

/// `depth(1)` 真的生效：取下来的裸仓里有 `shallow` 文件（祖先链断在取的那一个 commit 上），
/// 而那个 commit 的树照样可读（列技能读的是树，与历史深浅无关）。
#[tokio::test]
async fn fetch_is_shallow_and_leaves_a_shallow_file() {
    let mut fixture = RepoFixture::named(STEM).unwrap();
    fixture
        .add_file("skills/a/SKILL.md", &skill_md("a", "一", "正文"))
        .unwrap();
    fixture.commit("chore: 第一个").unwrap();
    fixture
        .add_file("skills/b/SKILL.md", &skill_md("b", "二", "正文"))
        .unwrap();
    let tip = fixture.commit("chore: 第二个").unwrap();
    let http = SmartHttp::serve(fixture.dir()).await.unwrap();

    let cache = cache_dir();
    let repo = Libgit2Repo::new(cache.clone())
        .with_base(&http.base())
        .unwrap();
    let id = RepoId::parse(SLUG).unwrap();
    let oid = Oid::parse(&tip).unwrap();
    let skills = repo.list_skills(&id, &oid).await.expect("列技能");
    assert_eq!(skills.len(), 2, "被取的 commit 树要可读：{skills:?}");

    // 缓存目录直接就是那个裸仓（`init_bare`），故 shallow 就在它根下
    let bare = cache.join(format!("{OWNER}__{STEM}__{tip}"));
    assert!(
        bare.join("shallow").is_file(),
        "depth(1) 应当留下 shallow 文件；实际目录：{:?}",
        std::fs::read_dir(&bare)
            .map(|d| d
                .filter_map(|e| e.ok())
                .map(|e| e.file_name())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
}

/// **列表钉住浏览那一刻的 commit，直到用户显式刷新**——"看到的 = 装到的"的落点。
#[tokio::test]
async fn list_skills_pins_the_commit_until_refresh() {
    let mut api = api_named(STEM, vec![SLUG.to_string()], |f| {
        f.add_file("skills/one/SKILL.md", &skill_md("one", "一", "正文"))
            .unwrap();
        f.commit("chore: 第一个技能").unwrap();
    })
    .await;

    let (status, first) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let pinned = first["commit"].as_str().unwrap().to_string();
    let listed_at = first["listed_at"].as_str().unwrap().to_string();
    assert_eq!(listed(&first).len(), 1);

    // 上游动了：新技能推上去了
    api.fixture
        .add_file("skills/two/SKILL.md", &skill_md("two", "二", "正文"))
        .unwrap();
    let new_tip = api.fixture.commit("feat: 第二个技能").unwrap();
    assert_ne!(new_tip, pinned);

    // 不刷新：还是那一份（SHA 与时间都不动，列表内容也不动）
    let (_, second) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(second["commit"].as_str().unwrap(), pinned);
    assert_eq!(second["listed_at"].as_str().unwrap(), listed_at);
    assert_eq!(listed(&second).len(), 1, "钉住的那一份里只有 one");

    // 刷新：换成新 tip，列出来的也跟着变
    let (status, third) = get(&api, "/market/skills?repo=acme%2Frepo&refresh=1").await;
    assert_eq!(status, StatusCode::OK, "{third}");
    assert_eq!(third["commit"].as_str().unwrap(), new_tip);
    assert_eq!(listed(&third).len(), 2);
}

/// 关键词过滤是**本地过滤**（不引 GitHub search API）：空关键词列全部，命中名字或描述。
#[tokio::test]
async fn list_skills_filters_locally() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (_, all) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(listed(&all).len(), 2);

    let (_, hit) = get(&api, "/market/skills?repo=acme%2Frepo&q=拷问").await;
    assert_eq!(
        listed(&hit)
            .iter()
            .map(|(_, n, _)| n.as_str())
            .collect::<Vec<_>>(),
        vec!["grilling"],
        "按描述命中"
    );

    let (_, none) = get(&api, "/market/skills?repo=acme%2Frepo&q=nope").await;
    assert_eq!(none["groups"].as_array().unwrap().len(), 0);
}

/// 未放行的仓**不进列表**（看不到装不上的东西），且此时**一次请求都没发出去**。
///
/// 这条同时是 E2E ⑫「未点添加之前零网络请求」的后端保证：仓名单为空 = 连 `head()` 都不发。
#[tokio::test]
async fn list_skills_refuses_unallowed_repos_without_any_request() {
    let api = api_with(Vec::new()).await;
    let (status, body) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "repo_not_allowed");
    assert!(
        body["error"].as_str().unwrap().contains("仓名单"),
        "报文要说清去哪加：{body}"
    );
    let http = api.http.as_ref().unwrap();
    assert!(
        http.requests().is_empty(),
        "未放行的仓不许发任何请求：{:?}",
        http.requests()
    );
}

// ─────────────────────────── 安装：钉住的那个 commit ───────────────────────────

/// 装到的是**被钉的那一个 commit**，不是 tip——这是 commit 锚的全部意义。
#[tokio::test]
async fn install_lands_the_pinned_commit_not_the_tip() {
    let mut api = api_named(STEM, vec![SLUG.to_string()], |f| {
        f.add_file(
            "skills/grill/SKILL.md",
            &skill_md("grill", "旧版", "旧正文"),
        )
        .unwrap();
        f.commit("chore: 旧版").unwrap();
    })
    .await;

    let (pinned, _) = list(&api).await;

    // 上游把正文换了（同一个技能目录，内容不同了）
    api.fixture
        .add_file(
            "skills/grill/SKILL.md",
            &skill_md("grill", "新版", "新正文"),
        )
        .unwrap();
    let tip = api.fixture.commit("feat: 改正文").unwrap();
    assert_ne!(tip, pinned);

    // 按**列表上那个** commit 装：拿到的必须是旧正文
    let (status, body) = install(&api, &pinned, "skills/grill", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "grill");
    let landed = std::fs::read_to_string(skills_root(&api).join("grill/SKILL.md")).unwrap();
    assert!(
        landed.contains("旧正文"),
        "装到的必须是被钉的那一份：{landed}"
    );
    assert!(!landed.contains("新正文"));

    // 来源记录写下了**那个** commit（不是 tip）
    let recorded = api
        .state
        .store
        .skill_source("grill")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.owner, OWNER);
    assert_eq!(recorded.repo, STEM);
    assert_eq!(recorded.commit_sha, pinned);
    assert_eq!(recorded.subpath, "skills/grill");
    assert_eq!(
        recorded.describe(),
        format!("{SLUG}@{}:skills/grill", &pinned[..7])
    );
}

/// 子树里的兄弟文件也真的落盘了（票 07 的展开依赖它）。
///
/// 这条打的是 `collect_files` 的**递归**：pulumi 的技能目录里 `agents/` 是子树，
/// 非递归会漏掉它。
#[tokio::test]
async fn install_lands_files_from_subtrees() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (commit, _) = list(&api).await;
    let (status, body) = install(&api, &commit, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        skills_root(&api).join("grilling/refs/notes.md").is_file(),
        "子树里的兄弟文件必须落盘"
    );
    let info = &body["skill"];
    assert!(info["sibling_count"].as_u64().unwrap() >= 1, "{body}");
}

/// 同名冲突：报文说出**装的是哪个仓哪个版本哪个子路径**；显式 `overwrite` 才覆盖。
#[tokio::test]
async fn conflict_names_the_recorded_origin_and_overwrite_replaces() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (commit, _) = list(&api).await;

    let (status, body) = install(&api, &commit, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 第二次：撞冲突，报文里是**来源记录**的仓坐标，不是技能根下的路径
    let (status, body) = install(&api, &commit, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("已存在"), "{msg}");
    assert!(msg.contains(&format!("{SLUG}@")), "要说清来自哪个仓：{msg}");
    assert!(msg.contains("skills/grilling"), "要说清哪个子路径：{msg}");
    assert!(
        !msg.contains("技能根下的"),
        "有记录时不该回落到路径形态：{msg}"
    );

    // 显式覆盖 → 走通
    let (status, body) = install(&api, &commit, "skills/grilling", true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 无记录时冲突报文**回落到路径形态**（手工拷进来 / 本地导入的技能没有记录）。
#[tokio::test]
async fn conflict_falls_back_to_the_path_form_without_a_record() {
    let api = api_with(vec![SLUG.to_string()]).await;
    // 本地导入一个与 fixture 同名的技能（这条路径不写来源记录）
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        json!({"paths": [write_local_skill(&api, "grilling", "本地正文")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(api
        .state
        .store
        .skill_source("grilling")
        .await
        .unwrap()
        .is_none());

    let (commit, _) = list(&api).await;
    let (status, body) = install(&api, &commit, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("技能根下的"), "无记录时回落路径形态：{msg}");
}

/// 卸载把来源记录一并删掉（否则下一次同名安装的冲突报文会报一个已经不存在的技能从哪来）。
#[tokio::test]
async fn uninstall_forgets_the_source_record() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (commit, _) = list(&api).await;
    let (status, body) = install(&api, &commit, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(api
        .state
        .store
        .skill_source("grilling")
        .await
        .unwrap()
        .is_some());

    let (status, _) = delete(&api, "/skills/grilling").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        api.state
            .store
            .skill_source("grilling")
            .await
            .unwrap()
            .is_none(),
        "卸载要一并删记录"
    );
}

/// 路径穿越在**远程路径**上同样被拒（`sanitize_dir` 与 `from_zip` 那两道门都在路上）。
#[tokio::test]
async fn install_refuses_a_traversing_subpath() {
    let api = api_with(vec![SLUG.to_string()]).await;
    let (commit, _) = list(&api).await;
    for bad in [
        "../etc",
        "skills/../../../etc",
        "skills/grilling/../../..",
        "/etc",
    ] {
        let (status, body) = install(&api, &commit, bad, false).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{bad} → {status} {body}"
        );
    }
    assert!(!api.state.home.root().join("etc").exists());
}

// ─────────────────────── 八类失败：每类一个 `kind` ───────────────────────

/// 契约层的核心断言：**状态码可以撞，`kind` 必须分得开**。
///
/// 界面按 `kind` 分支，不按状态码、更不按报文里的字样——`repo_not_found` 与
/// `commit_not_found` 都是 404，而前者要改仓名、后者要换 commit。
#[tokio::test]
async fn failure_classes_are_distinguishable_by_kind() {
    // 那个"不存在的仓名"必须**先放行**：不放行的话它在放行判定那一关就被拦成
    // `repo_not_allowed`，根本走不到"仓不存在"。两条判定分得开，正是本用例要钉的。
    let api = api_with(vec![SLUG.to_string(), "acme/nosuchrepo".to_string()]).await;
    let (good, _) = list(&api).await;

    // ① 仓不在白名单 → 400 / repo_not_allowed
    let (status, body) = get(&api, "/market/skills?repo=other%2Frepo").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "repo_not_allowed");

    // ② 仓不存在（fixture 只服务一个仓名，别的仓名 404）→ 404 / repo_not_found
    let (status, body) = get(&api, "/market/skills?repo=acme%2Fnosuchrepo").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "repo_not_found");
    assert!(
        body["error"].as_str().unwrap().contains("私有仓"),
        "要与「私有仓读不到」共用一个可操作的去处：{body}"
    );

    // ③ commit 不在历史里 → 404 / commit_not_found
    let bogus = "0".repeat(40);
    let (status, body) = install(&api, &bogus, "skills/grilling", false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "commit_not_found");

    // ④ 缩写 SHA → 也是 commit_not_found，但报文点明"要完整 40 位"。
    //    实测：7 位 SHA 会让 fetch 返回 `Ok` 却什么都不取——不拦就会报"装好了"而其实没装。
    let (status, body) = install(&api, &good[..7], "skills/grilling", false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "commit_not_found");
    assert!(body["error"].as_str().unwrap().contains("40"), "{body}");

    // ⑤ 这个仓里没有那个技能目录 → 404 / skill_not_found
    let (status, body) = install(&api, &good, "skills/nope", false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "skill_not_found");

    // ⑥ 请求体是残缺/畸形的仓名 → 也是 400 / repo_not_allowed（判定只有一处实现）
    let (status, body) = put(&api, "/market/repos", json!({"repos": ["a/../../../b"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// 连不上 → 502 / `market_network`（请求没问题，对面没应答——用户该做的是重试）。
#[tokio::test]
async fn network_failure_is_a_bad_gateway_with_its_own_kind() {
    let api = api_offline(vec![SLUG.to_string()]).await;
    let (status, body) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["kind"], "market_network");
    assert!(body["detail"].as_str().is_some(), "诊断串单列：{body}");
}

/// 超限 → 400 / `download_too_large`，且报文带**我们自己记的**数字与可操作的去向。
///
/// 这条同时钉住"边收边判"这条性质：`transfer_progress` 只在读块粒度上被叫
/// （实测最小约 64 KB），所以中断**只能发生在一块已经落下来之后**——没有"下载前先问大小"
/// 这条路（git 通道与 codeload 一样，只能边收边判）。
#[tokio::test]
async fn download_over_the_cap_is_reported_with_our_own_count() {
    // **不可压缩**的正文：可压缩的话 pack 会小到撞不上那个上限，用例就成了假绿。
    // 用一个简单 LCG 造伪随机 ASCII，别用 `"填充".repeat()` 那种（它压缩后只有几 KB）。
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let filler: String = (0..200_000)
        .map(|_| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'!' + ((seed >> 33) % 90) as u8)
        })
        .collect();

    let mut fixture = RepoFixture::named(STEM).unwrap();
    fixture
        .add_file("skills/big/SKILL.md", &skill_md("big", "大", &filler))
        .unwrap();
    fixture.commit("chore: 一个大文件").unwrap();
    let http = SmartHttp::serve(fixture.dir()).await.unwrap();
    let (home, state) = base_state(vec![SLUG.to_string()], &http.base(), Some(1024)).await;
    let router = build_router(state.clone());
    let api = Api {
        _home: home,
        fixture,
        http: Some(http),
        state,
        router,
    };

    let (status, body) = get(&api, "/market/skills?repo=acme%2Frepo").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "download_too_large");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("1024"), "须给出上限：{msg}");
    assert!(msg.contains("已收到"), "须给出我们自己记的字节数：{msg}");
    assert!(msg.contains("子目录"), "须给出可操作的去向：{msg}");
    // 中止有两种错误形态，其中一条（GIT_EUSER）的报文里**什么都没有**——
    // 故用户看到的数字只能来自我们自己记的那份，而诊断串单列在 detail 里
    assert!(body["detail"].as_str().is_some(), "{body}");
}

// ─────────────────── 一键安装（决策 181⑦，由决策 194 修订）───────────────────

/// 推荐清单里的技能带**定位字段**（仓 + 目录）：清单的全部意义是给还没装的人照着装。
#[tokio::test]
async fn recommendations_carry_their_locator() {
    let api = api_with(Vec::new()).await;
    let (status, body) = get(&api, "/skills/recommendations").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let grilling = body["stages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["skills"].as_array().unwrap())
        .find(|s| s["name"] == "grilling")
        .expect("grilling 应当在清单里");
    assert_eq!(grilling["repo"], LISTED_SLUG);
    assert_eq!(grilling["dir"], "skills/productivity/grilling");
    assert_eq!(grilling["installed"], false);
}

/// 一键安装：按清单的定位取仓 → head → 读目录 → 落盘 → 写进该阶段配置 → 返回三项预览。
///
/// fixture 顶替的**就是清单指向的那个仓**（`mattpocock/skills`），故这条走的是完整的
/// 真链路，只有"仓里有这些内容"是本地造的。
#[tokio::test]
async fn one_click_install_lands_the_skill_and_writes_the_stage_config() {
    let api = api_named("skills", vec![LISTED_SLUG.to_string()], |f| {
        f.add_file(
            "skills/productivity/grilling/SKILL.md",
            &skill_md(
                "grilling",
                "拷问设计树",
                "第一行\n第二行 curl https://evil.example",
            ),
        )
        .unwrap();
        f.commit("chore: 清单指的那一份").unwrap();
    })
    .await;

    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "grilling");
    assert!(skills_root(&api).join("grilling/SKILL.md").is_file());

    // 写进配置的声明只能是 name 模式 + 未信任（新装技能还没被人确认过）
    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    let decls = agentpipeline_core::config::declared_skill_decls(&stored);
    assert_eq!(decls.len(), 1, "{decls:?}");
    assert_eq!(decls[0].1.name, "grilling");
    assert!(!decls[0].1.trusted, "新装技能默认未受信任");

    // 三项预览齐备（决策 181③：特征命中要摆在眼前再决定要不要启用）
    assert!(
        body["preview"]["recommendations"].as_array().is_some(),
        "{body}"
    );
    assert!(
        body["preview"]["declarations"].as_array().is_some(),
        "{body}"
    );
    assert!(
        body["preview"]["body_available"].as_bool().unwrap(),
        "{body}"
    );
    // 正文里那个 `curl` 要**逐行**命中（行号 + 原行），不是一句"有风险"
    let hits = body["preview"]["features"]["hits"].as_array().unwrap();
    assert!(!hits.is_empty(), "{body}");
    let curl = hits
        .iter()
        .find(|h| h["text"].as_str().unwrap().contains("curl"))
        .expect("curl 那行要命中");
    assert_eq!(curl["kind"], "network");
    assert!(
        curl["line"].as_u64().unwrap() >= 1,
        "命中要带从 1 起算的行号：{body}"
    );

    // 装了就记来源（后续的冲突报文与"是不是同一份"都靠它）
    let recorded = api
        .state
        .store
        .skill_source("grilling")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.slug(), LISTED_SLUG);
    assert_eq!(recorded.subpath, "skills/productivity/grilling");
}

/// 已装 + 没有来源记录 → **跳过下载只写配置**，一次网络请求都不发（决策 181⑦）。
///
/// 这条路的可离线性是它存在的理由：本用例的仓访问层指向一个关闭的端口，
/// 一旦它真去取仓，这里就会以 502 失败。
#[tokio::test]
async fn one_click_install_skips_the_download_when_already_installed() {
    let api = api_offline(vec![LISTED_SLUG.to_string()]).await;
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        json!({"paths": [write_local_skill(&api, "grilling", "本地正文")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["note"].as_str().unwrap().contains("未重新下载"),
        "跳过要说出声，不能静默：{body}"
    );
    let content = std::fs::read_to_string(skills_root(&api).join("grilling/SKILL.md")).unwrap();
    assert!(
        content.contains("本地正文"),
        "跳过的意思是不动它：{content}"
    );
}

/// 已装但**确知**来自别的仓 → **不跳过**，撞同名冲突门由用户裁决。
///
/// 这是"不静默换成旧版、也不静默升级"的落点：那一份确知不是清单指的这份，就不能假装是。
#[tokio::test]
async fn one_click_install_does_not_silently_replace_a_differently_sourced_copy() {
    // fixture 顶替**清单指向的那个仓**：这条路的落点是"从清单那份下载下来之后撞同名门"，
    // 故它必须真能下载——不然测的就不是"不静默替换"，而是"下载失败"。
    let api = api_named("skills", vec![LISTED_SLUG.to_string()], |f| {
        f.add_file(
            "skills/productivity/grilling/SKILL.md",
            &skill_md("grilling", "拷问设计树", "清单那一份的正文"),
        )
        .unwrap();
        f.commit("chore: 清单指的那一份").unwrap();
    })
    .await;

    // 技能根里先放一份同名技能（走本地导入，不碰网络），再把它记成"来自别的仓"
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        json!({"paths": [write_local_skill(&api, "grilling", "别的来源的正文")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    api.state
        .store
        .record_skill_source(&SkillSource {
            name: "grilling".into(),
            owner: "someone-else".into(),
            repo: "other-repo".into(),
            commit_sha: "a".repeat(40),
            subpath: "skills/grilling".into(),
            installed_at: api.state.store.now().to_rfc3339(),
        })
        .await
        .unwrap();
    let before = std::fs::read_to_string(skills_root(&api).join("grilling/SKILL.md")).unwrap();

    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "不许静默换掉：{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(
        msg.contains("someone-else/other-repo"),
        "要说清它现在来自哪：{msg}"
    );
    // 冲突就是**没动它**：报文说了要来裁决，正文就不该已经变了
    let after = std::fs::read_to_string(skills_root(&api).join("grilling/SKILL.md")).unwrap();
    assert_eq!(before, after, "撞了冲突门就先别落盘");
}

/// 一键安装带 `overwrite` 时**不是"跳过下载"**：回来源仓重新取一份（`note` 为空），
/// 而阶段配置里的条目仍是一条（覆盖换的是技能根里的字节，不是配置条目）。
///
/// 从 `api_contract.rs` 搬过来的一条：那里注入的仓访问层指向一个关闭的端口（零网络），
/// 而这条必须真的能下载——它的落点就是"下载与覆盖"，故与 fixture 同住。
#[tokio::test]
async fn one_click_install_with_overwrite_refetches_and_keeps_one_declaration() {
    let api = api_named("skills", vec![LISTED_SLUG.to_string()], |f| {
        f.add_file(
            "skills/productivity/grilling/SKILL.md",
            &skill_md("grilling", "拷问设计树", "仓里的正文"),
        )
        .unwrap();
        f.commit("chore: 清单指的那一份").unwrap();
    })
    .await;
    // 先本地放一份同名技能（没有来源记录 → 算"就是清单那份"）并启用：这条不下载
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        json!({"paths": [write_local_skill(&api, "grilling", "本地正文")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["note"].as_str().unwrap().contains("未重新下载"),
        "第一次是跳过：{body}"
    );

    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling", "overwrite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["note"].is_null(), "显式覆盖不是「跳过下载」：{body}");
    assert_eq!(
        body["stage_config"]["skills_json"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "重复启用不得产生第二条声明：{body}"
    );
    // 字节真换了：来源仓那一份覆盖了本地那份
    let content = std::fs::read_to_string(skills_root(&api).join("grilling/SKILL.md")).unwrap();
    assert!(
        content.contains("仓里的正文"),
        "覆盖要把技能根里那份换成来源仓的：{content}"
    );
    assert!(!content.contains("本地正文"), "旧正文不许留残骸：{content}");
}

/// 一键安装的失败按八类可归因，且**不留半条配置**。
#[tokio::test]
async fn one_click_install_leaves_no_half_written_config_on_failure() {
    let api = api_with(Vec::new()).await;
    let (status, body) = post(
        &api,
        "/skills/install",
        json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "repo_not_allowed", "{body}");
    assert!(body["error"].as_str().unwrap().contains("仓名单"), "{body}");
    assert!(
        api.state
            .store
            .get_stage_config("architect-design")
            .await
            .unwrap()
            .is_none(),
        "装不上就不该留下半条配置"
    );
}

// ─────────────────────────────── 工具 ───────────────────────────────

/// 在临时目录里写一个本地技能目录（供"本地导入"那条路用），返回它的路径。
fn write_local_skill(api: &Api, name: &str, body: &str) -> std::path::PathBuf {
    let dir = api
        .state
        .home
        .root()
        .join(format!("local-src-{name}"))
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(name, "本地装的", body)).unwrap();
    dir
}
