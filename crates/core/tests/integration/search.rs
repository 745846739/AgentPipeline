//! L2 集成：`search_content`——只读层的内容检索（决策 267，票 foreman-within-boundary 01）。
//!
//! 判据落在副作用与台账上（与 `readonly.rs` / `web_fetch.rs` 同一取证手法）：
//!
//! 1. 域内按正则命中：回执带「相对路径:行号:行文本」，台账落一行（`search_content …`、退出 0）；
//! 2. `data/` 前缀在**行走之前**被域策略拒（206 同一条 `check_read`，留行、退出码留空）；
//! 3. 域外路径（绝对路径 `/etc`）同样拒；
//! 4. 坏正则是形状错不是尝试——`Validation` 回给模型，**不落行**（`ask` 同款）；
//! 5. 命中行数有上限，超了带截断标注（transcript 12k 口径的前一道闸）；
//! 6. 二进制文件（首块含 NUL）跳过——不进对话上下文；
//! 7. 不跟符号链接：指向域外的链接一步都走不进去（越域 + 环，两个理由）；
//! 8. 档位三档都广告它（237 判据：改不了任何东西），值守轮 deny 清单**不摘**它
//!    （`run_readonly` 先例：夜里找证据不需要人在场）。

use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::{
    foreman_available_tools, foreman_available_tools_except, FOREMAN_WATCH_TOOL_DENY,
};
use agentpipeline_core::types::{CommandSource, EnvMode, Node, Stage};
use agentpipeline_core::Error;
use testkit::{ManualClock, RecordingKiller, TestHome};

struct Fixture {
    home: TestHome,
    store: agentpipeline_core::storage::Store,
    session_id: String,
}

async fn fixture() -> Fixture {
    let home = TestHome::new().unwrap();
    let store = agentpipeline_core::storage::Store::open(
        home.home().clone(),
        Arc::new(ManualClock::fixed()),
    )
    .await
    .unwrap();
    let session_id = store.create_foreman_session("搜索").await.unwrap().id;
    Fixture {
        home,
        store,
        session_id,
    }
}

impl Fixture {
    /// 与 `foreman_tooling` 同源的构造（域 = 家目录根 + 记录器接上）。
    fn executor(&self) -> ToolExecutor {
        ToolExecutor::new(
            self.home.home().clone(),
            agentpipeline_core::agent::file_policy::foreman_file_policy(self.home.home().root()),
            Settings::default(),
            Arc::new(RecordingKiller::new()),
        )
        .with_recorder(Arc::new(self.store.clone()))
        .with_env_mode(EnvMode::Auto)
    }

    fn ctx(&self) -> ToolCallContext {
        ToolCallContext {
            task_id: String::new(),
            session_id: Some(self.session_id.clone()),
            stage: Stage::Init,
            node: Node::Execute,
            worktree_path: self.home.home().root().to_path_buf(),
            task_dir: self.home.home().root().to_path_buf(),
            run_id: None,
            command_source: CommandSource::Agent,
            default_cwd: None,
        }
    }

    /// 写一个域内文件（相对家目录根）。
    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.home.home().root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

fn search(pattern: &str) -> ToolCall {
    search_in(pattern, None)
}

fn search_in(pattern: &str, path: Option<&str>) -> ToolCall {
    let mut args = serde_json::json!({ "pattern": pattern });
    if let Some(p) = path {
        args["path"] = serde_json::json!(p);
    }
    ToolCall {
        id: "c".into(),
        name: "search_content".into(),
        arguments: args.to_string(),
    }
}

/// 台账里那一行（被拒的尝试也要有）。
async fn last_command(f: &Fixture) -> agentpipeline_core::types::NodeCommand {
    f.store
        .list_foreman_commands(&f.session_id)
        .await
        .unwrap()
        .pop()
        .expect("应当落了一行命令台账")
}

#[tokio::test]
async fn a_content_search_finds_matches_in_the_home_domain_and_lands_in_the_ledger() {
    let f = fixture().await;
    f.write("notes/hello.txt", b"line1\nneedle-42 here\nline3\n");
    f.write("notes/other.txt", b"nothing to see\n");

    let out = f
        .executor()
        .execute(&search(r"needle-\d+"), &f.ctx())
        .await
        .expect("域内搜索应当成功");
    assert!(
        out.content.contains("notes/hello.txt") && out.content.contains(":2:"),
        "回执要带相对路径与行号：{}",
        out.content
    );
    assert!(out.content.contains("needle-42"), "{}", out.content);
    assert!(
        !out.content.contains("nothing to see"),
        "不命中就不该出现：{}",
        out.content
    );

    let row = last_command(&f).await;
    assert!(
        row.command.starts_with("search_content"),
        "台账那行的命令形状：{row:?}"
    );
    assert_eq!(row.exit_code, Some(0), "{row:?}");
}

#[tokio::test]
async fn a_data_prefix_search_is_refused_by_the_same_domain_rule() {
    let f = fixture().await;
    f.write("data/secret.env", b"needle-token\n");

    let err = f
        .executor()
        .execute(&search_in(r"needle-\w+", Some("data")), &f.ctx())
        .await
        .expect_err("data/ 前缀必须被域策略拒");
    match err {
        Error::PolicyDenied(msg) => {
            assert!(msg.contains("data"), "报错要说清是域的事：{msg}")
        }
        other => panic!("应当是 PolicyDenied，收到 {other:?}"),
    }
    let row = last_command(&f).await;
    assert_eq!(
        row.exit_code, None,
        "没跑起来就没有退出码（refuse_readonly 同口径）：{row:?}"
    );
}

#[tokio::test]
async fn a_path_outside_the_home_domain_is_refused() {
    let f = fixture().await;

    let err = f
        .executor()
        .execute(&search_in(".*", Some("/etc")), &f.ctx())
        .await
        .expect_err("域外绝对路径必须被拒");
    assert!(
        matches!(err, Error::PolicyDenied(_)),
        "应当是 PolicyDenied，收到 {err:?}"
    );
    assert_eq!(last_command(&f).await.exit_code, None);
}

#[tokio::test]
async fn a_malformed_regex_is_a_validation_error_and_lands_nothing() {
    let f = fixture().await;
    f.write("a.txt", b"whatever\n");

    let err = f
        .executor()
        .execute(&search("["), &f.ctx())
        .await
        .expect_err("坏正则要回给模型");
    assert!(
        matches!(err, Error::Validation(_)),
        "形状错是 Validation，收到 {err:?}"
    );
    assert!(
        f.store
            .list_foreman_commands(&f.session_id)
            .await
            .unwrap()
            .is_empty(),
        "没发生过尝试，就不该有台账行"
    );
}

#[tokio::test]
async fn matches_are_capped_with_a_truncation_note() {
    let f = fixture().await;
    let mut body = String::new();
    for i in 1..=300 {
        body.push_str(&format!("hit-{i}\n"));
    }
    f.write("many.txt", body.as_bytes());

    let out = f
        .executor()
        .execute(&search(r"hit-\d+"), &f.ctx())
        .await
        .expect("应当成功（截断不是失败）");
    assert!(out.content.contains("hit-200"), "{}", out.content);
    assert!(
        !out.content.contains("hit-201"),
        "201 行不该出现在回执里：{}",
        out.content
    );
    assert!(out.content.contains("截断"), "{}", out.content);
    assert_eq!(last_command(&f).await.exit_code, Some(0));
}

#[tokio::test]
async fn binary_files_are_skipped() {
    let f = fixture().await;
    f.write("blob.bin", b"bin\0ary needle-77 tail");

    let out = f
        .executor()
        .execute(&search("needle-77"), &f.ctx())
        .await
        .expect("跳过二进制不是错误");
    assert!(
        !out.content.contains("needle-77"),
        "二进制内容不进回执：{}",
        out.content
    );
}

#[tokio::test]
async fn symlinks_pointing_outside_are_not_followed() {
    let f = fixture().await;
    // 域外目标：第二个 TestHome（独立临时目录）里放一个含标记的文件。
    let outside = TestHome::new().unwrap();
    std::fs::write(
        outside.home().root().join("leak.txt"),
        b"needle-outside-999\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.home().root().join("leak.txt"),
        f.home.home().root().join("sneaky.txt"),
    )
    .unwrap();

    let out = f
        .executor()
        .execute(&search("needle-outside"), &f.ctx())
        .await
        .expect("链接走不进去不是错误");
    assert!(
        !out.content.contains("needle-outside"),
        "不跟符号链接——域外内容一步都不许走：{}",
        out.content
    );
}

#[tokio::test]
async fn an_oversized_file_is_read_only_up_to_the_cap_and_says_so() {
    let f = fixture().await;
    // 单文件上限 1 MiB：把标记放到界外，前 1 MiB 里只有填充
    let mut body = vec![b'a'; 1024 * 1024 + 512];
    body.extend_from_slice(b"\nneedle-past-cap\n");
    f.write("huge.txt", &body);

    let out = f
        .executor()
        .execute(&search("needle-past-cap"), &f.ctx())
        .await
        .expect("超限截断不是错误");
    assert!(
        !out.content.contains("needle-past-cap") || out.content.contains("只读了每个文件的前"),
        "界外标记不该命中：{}",
        out.content
    );
    assert!(
        out.content.contains("只读了每个文件的前"),
        "单文件超限要带截断标注（267②，不许静默）：{}",
        out.content
    );
    assert_eq!(last_command(&f).await.exit_code, Some(0));
}

#[tokio::test]
async fn the_tiers_leave_it_alone_and_the_watch_deny_list_keeps_it() {
    // 档位三档都广告（237 判据：改不了任何东西）；值守轮 deny 清单**不摘**它——
    // 与 `run_readonly` 同款：自主轮夜里找证据不需要人在场（267④）。
    for mode in [EnvMode::Auto, EnvMode::Ask, EnvMode::Deny] {
        let avail = foreman_available_tools(mode);
        assert!(
            avail.contains(&"search_content"),
            "{mode:?} 下都要广告 search_content：{avail:?}"
        );
    }
    let watch = foreman_available_tools_except(EnvMode::Ask, &FOREMAN_WATCH_TOOL_DENY);
    assert!(
        watch.contains(&"search_content"),
        "值守轮也要能找内容（run_readonly 先例）：{watch:?}"
    );
}
