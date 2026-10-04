//! L2 集成：`ToolExecutor::runner_for` 文档注释里的「测试隔离教训」段（票 runner-offload/03 补遗）。
//!
//! 本任务（design.md）是**纯文档注释改动**——在 `crates/core/src/agent/tools.rs` 的
//! `runner_for` 文档注释里补一段「**测试隔离教训**」，说明闸门注入的 `CARGO_TARGET_DIR`
//! 会顺着环境传进测试子进程，故测试断言必须 hermetic（对执行器自己 home 的
//! `shared_target_path()` 具体路径断言），不能用可被外层环境变量误伤的 `"shared-target"`
//! 子串断言。
//!
//! 注释不参与编译语义，**行为断言碰不到它**——所以这一份用**读源码文本**来钉（与
//! `tools.rs::ledger_previews_are_built_only_by_the_byte_capped_helper`、`app` 的
//! `api_contract.rs` 读源码那几条同一个姿态）：`include_str!` 把文件编进测试二进制，
//! 且 rustc 的 dep-info 会跟着它走，**注释一改就重编重跑**，不会拿旧二进制蒙混。
//!
//! 覆盖的场景（test-scenarios.md）：
//!
//! - 场景 1（注释内容与位置，AC-1）：三要点齐备、插在「变量对非 cargo 命令无害。」与
//!   「**并发语义**」之间、只增不改；
//! - 场景 2（注释与代码对齐，AC-1）：闸门两处接线 + 执行器侧落点 + 被引用的 hermetic
//!   用例都真实存在，注释不悬空；
//! - 场景 4（回归防线，AC-2）：**真跑一次**——外层 `CARGO_TARGET_DIR` 污染下，hermetic
//!   断言仍绿，而旧子串写法会被误伤；
//! - 场景 7（rustdoc 边界，AC-1 / AC-4）：引用是反引号代码片段，不是 intra-doc link。
//!
//! 场景 3 / 5 / 6（提交落点、工作区清洁、外发 clippy）是**流程卫生证据**，由 test-report.md
//! 里的命令台账承载（`git show --stat` / `git status` / 提交信息），不在这里造 git-history
//! 断言——CI 的 checkout 是 depth=1，按历史找提交会假红（口径见 `docs/testing.md` §6）。

use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::file_policy::FileToolPolicy;
use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};
use agentpipeline_core::config::Settings;
use agentpipeline_core::home::Home;
use agentpipeline_core::types::{CommandSource, Node, Stage};
use testkit::RecordingKiller;

// ─────────────────────────── 源码文本（编译期嵌入，dep-info 跟踪）───────────────────────────

const TOOLS_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/agent/tools.rs"));
const EXECUTOR_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/pipeline/executor.rs"
));
const REPAIR_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/pipeline/repair.rs"
));

/// 被注释点名引用的 hermetic 用例（正例指引）。
const REFERENCED_CASE: &str = "session_commands_do_not_get_the_shared_target_var";

/// 取 `fn runner_for` 上方那段**连续**的 `///` 文档注释（原样，含行首缩进与 `///`）。
fn runner_for_doc_block(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let fn_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .starts_with("fn runner_for(&self, ctx: &ToolCallContext)")
        })
        .expect("tools.rs 里应有 `fn runner_for` 定义");
    let mut start = fn_idx;
    while start > 0 && lines[start - 1].trim_start().starts_with("///") {
        start -= 1;
    }
    assert!(
        start < fn_idx,
        "`fn runner_for` 上方应有文档注释——本票的改动落点就是它"
    );
    lines[start..fn_idx].join("\n")
}

/// 取文件的**生产段**（`#[cfg(test)] mod tests` 之前的部分）。
///
/// 不能用「第一个 `#[cfg(test)]`」来切：`executor.rs` 里另有两处 `#[cfg(test)]` 标在
/// **生产函数**上（`held_by_human` / `cancel_signal`，`pub(crate)` 给 app 层测试用），
/// 按第一处切会把它们的正文连带切掉、也把后面真正的注入点一并切掉。
fn production_of(source: &str) -> &str {
    source
        .find("\n#[cfg(test)]\nmod tests {")
        .map(|i| &source[..i])
        .unwrap_or(source)
}

/// 往下取到最近的 UTF-8 边界（源码里全是中文，字节偏移不能直接切）。
fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// 往上取到最近的 UTF-8 边界。
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 取源码里 `needle` 起、到第一个 4 空格缩进的 `}` 为止的片段（函数体 / 用例体的粗切）。
fn body_of(source: &str, needle: &str) -> String {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("源码里找不到 `{needle}`"));
    let rest = &source[start..];
    const END: &str = "\n    }";
    let end = rest.find(END).map(|i| i + END.len()).unwrap_or(rest.len());
    rest[..end].to_string()
}

/// 文档注释里「**测试隔离教训**」段全文（段首那一行起、到 `**并发语义**` 前）。
///
/// 起点对齐到**行首**（保留行首缩进与 `///` 前缀），这样调用方可以逐行核「全是注释行」。
fn lesson_section(doc: &str) -> String {
    let marker = doc
        .find("**测试隔离教训**")
        .expect("runner_for 文档注释里应有「**测试隔离教训**」段首标记");
    let line_start = doc[..marker].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let rest = &doc[line_start..];
    let end = rest
        .find("**并发语义**")
        .expect("「**并发语义**」段应仍在（它在新段之后）");
    rest[..end].to_string()
}

// ─────────────────────────────── 场景 1：注释内容与位置（AC-1）───────────────────────────────

/// 三要点齐备、位置正确、只增不改。
#[test]
fn the_lesson_section_has_all_three_points_in_the_right_place() {
    let doc = runner_for_doc_block(TOOLS_SRC);

    // 位置①：段首标记带出处，且排在「变量对非 cargo 命令无害。」之后。
    let header = doc
        .find("**测试隔离教训**")
        .expect("段首标记「**测试隔离教训**」应在");
    let anchor = doc
        .find("变量对非 cargo 命令无害。")
        .expect("上一段的收句「变量对非 cargo 命令无害。」应逐字仍在（只增不改）");
    assert!(
        anchor < header,
        "新段应插在「变量对非 cargo 命令无害。」**之后**：anchor={anchor} header={header}"
    );
    assert!(
        doc.contains("**测试隔离教训**（2026-10-03 闸门实测）"),
        "段首应带 2026-10-03 闸门实测的出处说明"
    );

    // 位置②：「并发语义」段仍在新段之后（未把要点写进别处、也未顶掉它）。
    let concurrency = doc.find("**并发语义**").expect("「**并发语义**」段应仍在");
    assert!(header < concurrency, "新段应在「**并发语义**」**之前**");

    let lesson = lesson_section(&doc);

    // 要点①：闸门侧同一条接线**也注入**该变量。
    assert!(
        lesson.contains("闸门侧") && lesson.contains("同一条接线也注入"),
        "要点①（闸门侧同一条接线也注入）缺失：{lesson}"
    );
    // 要点②：该变量**顺着环境传进测试子进程**（测试二进制）。
    assert!(
        lesson.contains("顺着环境传进测试二进制"),
        "要点②（顺着环境传进测试二进制）缺失：{lesson}"
    );
    // 要点③：对**执行器自己 home 的 `shared_target_path()` 具体路径**断言（hermetic），
    // 勿用 `"shared-target"` 子串断言。
    assert!(
        lesson.contains("shared_target_path()") && lesson.contains("具体路径"),
        "要点③（对 shared_target_path() 具体路径断言）缺失：{lesson}"
    );
    assert!(
        lesson.contains("hermetic"),
        "要点③应点明这是 hermetic（对具体路径）的做法：{lesson}"
    );
    assert!(
        lesson.contains("`\"shared-target\"`") && lesson.contains("子串断言"),
        "要点③应明确反对 `\"shared-target\"` 子串断言（反例形态）：{lesson}"
    );
    assert!(
        lesson.contains("外层环境变量误伤"),
        "要点③应说清为何不用子串断言（可被外层环境变量误伤）：{lesson}"
    );
    // 引用正例指引（那个用例名）。
    assert!(
        lesson.contains(&format!("`{REFERENCED_CASE}`")),
        "教训段应引用 hermetic 正例 `{REFERENCED_CASE}`：{lesson}"
    );

    // 「并发语义」段（超时口径）原封不动。
    for kept in [
        "零进度排队",
        "tool_timeout_sec",
        "test_command_timeout_sec",
        "600s",
    ] {
        assert!(
            doc.contains(kept),
            "「并发语义」段的既有表述 `{kept}` 不得被本次改动触碰"
        );
    }
}

/// 新段只出现在 `runner_for` 的文档注释里（不在 `runner()` / `offload_run()` 的注释）。
#[test]
fn the_lesson_section_is_written_once_and_only_in_runner_for_doc() {
    assert_eq!(
        TOOLS_SRC.matches("**测试隔离教训**").count(),
        1,
        "「**测试隔离教训**」段应**恰有一处**——写进别处（runner() / offload_run() 注释）即偏离设计"
    );
    let doc = runner_for_doc_block(TOOLS_SRC);
    assert!(
        doc.contains("**测试隔离教训**"),
        "那一处必须落在 `fn runner_for` 上方的文档注释里"
    );
}

// ────────────────── 场景 3（AC-2 的源码侧形态）：只增注释行、签名逐字未动 ──────────────────

/// 改动是**只增文档注释行**：新增段每一行都以 `///` 开头，函数签名逐字未变。
#[test]
fn the_change_adds_only_doc_comment_lines_and_keeps_the_signature_verbatim() {
    // 签名逐字未变（本票明确「不改函数签名」）。
    assert!(
        TOOLS_SRC.contains(
            "fn runner_for(&self, ctx: &ToolCallContext) -> crate::exec::CommandRunner {"
        ),
        "`fn runner_for` 的签名应逐字未变"
    );

    // 新段全是 `///` 注释行——没有夹带代码 / 测试逻辑行。
    let lesson = lesson_section(&runner_for_doc_block(TOOLS_SRC));
    let mut lines = 0usize;
    for line in lesson.lines() {
        let trimmed = line.trim_start();
        assert!(
            trimmed.starts_with("///"),
            "「测试隔离教训」段里出现了非注释行：{line}"
        );
        if !trimmed.trim_start_matches("///").trim().is_empty() {
            lines += 1;
        }
    }
    assert!(
        lines >= 5,
        "教训段应是一小段（≥5 行实质注释），实得 {lines} 行"
    );
    for banned in [
        "fn ",
        "let ",
        "runner.with_extra_env",
        "assert",
        "#[test]",
        "#[tokio::test]",
    ] {
        assert!(
            !lesson.contains(banned),
            "纯注释改动里不得出现代码 / 测试逻辑行 `{banned}`：{lesson}"
        );
    }

    // 上一段的句子（只增不改的另一面：它们逐字还在）。
    let doc = runner_for_doc_block(TOOLS_SRC);
    for kept in [
        "任务上下文里的命令附加**共享构建缓存**",
        "变量对非 cargo 命令无害。",
    ] {
        assert!(doc.contains(kept), "既有句子 `{kept}` 应逐字未变");
    }
}

// ─────────────────── 场景 2：注释所述事实与既有代码对齐、引用不悬空（AC-1）───────────────────

/// 注释①（闸门侧同一条接线也注入）在 `executor.rs` / `repair.rs` 两处都有代码事实。
#[test]
fn the_gate_side_injection_the_lesson_points_at_really_exists_in_both_gates() {
    for (name, source) in [
        ("pipeline/executor.rs", EXECUTOR_SRC),
        ("pipeline/repair.rs", REPAIR_SRC),
    ] {
        let production = production_of(source);
        let idx = production
            .find("\"CARGO_TARGET_DIR\".to_string()")
            .unwrap_or_else(|| {
                panic!("{name} 的生产段应有 `CARGO_TARGET_DIR` 注入（注释①的落点）")
            });
        // `with_extra_env` 在键**之前**（`.with_extra_env(vec![("CARGO_TARGET_DIR".to_string(),`），
        // `shared_target_path()` 在键**之后**，故窗口取键两侧。
        let before = &production[floor_char_boundary(production, idx.saturating_sub(200))..idx];
        let after =
            &production[idx..ceil_char_boundary(production, (idx + 200).min(production.len()))];
        assert!(
            before.contains("with_extra_env"),
            "{name} 的注入应经 `with_extra_env` 落到子进程环境：{before}"
        );
        assert!(
            after.contains("shared_target_path()"),
            "{name} 的注入值应来自 `shared_target_path()`：{after}"
        );
    }
}

/// 闸门侧的注入点**恰是注释点名的这两个**（脚本横查，防「新增接线点使注释失实」）。
#[test]
fn the_gate_side_injection_points_are_exactly_the_two_the_lesson_names() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src/pipeline 应可读") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("\"CARGO_TARGET_DIR\".to_string()") {
            found.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    found.sort();
    assert_eq!(
        found,
        vec!["executor.rs".to_string(), "repair.rs".to_string()],
        "闸门侧注入点变了 → 上面那段注释的「各自接线」说法就不完整了，需同步更新；实得 {found:?}"
    );
}

/// 注释②③的落点（`runner_for` 的 `session_id.is_none()` 分支）存在。
#[test]
fn the_executor_side_injection_point_the_lesson_points_at_really_exists() {
    let body = body_of(TOOLS_SRC, "fn runner_for(&self, ctx: &ToolCallContext)");
    assert!(
        body.contains("if ctx.session_id.is_none()"),
        "注释放点应是 `runner_for` 的 `session_id.is_none()` 分支：{body}"
    );
    assert!(
        body.contains("self.home.shared_target_path()"),
        "该分支应把 `self.home.shared_target_path()` 注入 `CARGO_TARGET_DIR`：{body}"
    );
    assert!(
        body.contains("\"CARGO_TARGET_DIR\".to_string()"),
        "该分支注入的键应是 `CARGO_TARGET_DIR`：{body}"
    );
}

/// 被注释点名的用例真实存在、且是 hermetic 写法（注释引用不悬空）。
#[test]
fn the_referenced_case_exists_and_asserts_on_the_concrete_path() {
    // 取 `#[cfg(test)] mod tests` 之后的**全部**内容（不能用「第二个 `#[cfg(test)]`」
    // 来切——本文件里另有两处 `#[cfg(test)]` 写在字符串字面量里，会把它截在半途）。
    let test_section = TOOLS_SRC
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map(|(_, rest)| rest)
        .expect("tools.rs 应有 `#[cfg(test)] mod tests` 段");
    assert!(
        test_section.contains(&format!("async fn {REFERENCED_CASE}(")),
        "被注释引用的用例 `{REFERENCED_CASE}` 必须真实存在——否则注释引用悬空"
    );

    let case = body_of(test_section, &format!("async fn {REFERENCED_CASE}("));
    // hermetic：对**执行器自己 home 的共享路径**断言（具体路径，不是子串）。
    assert!(
        case.contains("s.home.shared_target_path().display().to_string()"),
        "该用例应对 `shared_target_path()` 具体路径断言：{case}"
    );
    assert!(
        case.contains("!out.content.contains(&injected)"),
        "该用例应是「不得含执行器自己的共享路径」的**否定**断言：{case}"
    );
    // 且**不是**旧写法（`!contains("shared-target")`）——那正是注释警示的反例。
    assert!(
        !case.contains("!out.content.contains(\"shared-target\")"),
        "该用例不该退回旧子串写法（注释的「正例指引」也随之失效）：{case}"
    );
}

// ─────────────────── 场景 4：外层环境变量污染下的 hermetic 回归（AC-2）────────────────────

/// 本文件里动进程环境的那几条测试互斥（`set_var` 是全进程的）。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 外层 `CARGO_TARGET_DIR` 污染的作用域守卫：装上 → 跑 → Drop 还回去。
///
/// 它模拟的是**闸门自己那层环境**（106 的 develop 闸门按票 03 接线，给 `cargo test`
/// 进程注入 `CARGO_TARGET_DIR=<闸门 home>/shared-target`）。测试子进程会**原样继承**它——
/// 这正是注释里那条教训的成因。
struct OuterTargetDir {
    previous: Option<std::ffi::OsString>,
}

impl OuterTargetDir {
    fn install(value: &str) -> Self {
        let previous = std::env::var_os("CARGO_TARGET_DIR");
        std::env::set_var("CARGO_TARGET_DIR", value);
        OuterTargetDir { previous }
    }
}

impl Drop for OuterTargetDir {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: Home,
    executor: ToolExecutor,
    ctx: ToolCallContext,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::new(tmp.path().join("home"));
    home.ensure_dirs().unwrap();
    let worktree = home.worktree_path("t1");
    let task_dir = home.task_dir("t1");
    home.ensure_task_dirs("t1").unwrap();

    let policy = FileToolPolicy::new(vec![worktree.clone(), task_dir.clone()]);
    let executor = ToolExecutor::new(
        home.clone(),
        policy,
        Settings::default(),
        Arc::new(RecordingKiller::new()),
    );
    let ctx = ToolCallContext {
        task_id: "t1".into(),
        session_id: None,
        stage: Stage::Develop,
        node: Node::Execute,
        worktree_path: worktree.clone(),
        task_dir: task_dir.clone(),
        run_id: Some(1),
        command_source: CommandSource::Agent,
        default_cwd: Some(worktree),
    };
    Fixture {
        _tmp: tmp,
        home,
        executor,
        ctx,
    }
}

fn call(command: &str) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({ "command": command }).to_string(),
    }
}

/// 外层环境变量污染**不影响判定**——hermetic 断言（对执行器自己 home 的具体路径）照旧成立。
///
/// 污染路径含 `shared-target` 子串但与执行器 home 不同：旧写法
/// `!out.content.contains("shared-target")` 会把它当成「拿到了注入」而**假红**。
#[tokio::test]
// `CARGO_TARGET_DIR` 是进程级环境，这两条必须互斥跑（`set_var` 影响整个测试进程）；
// 拿锁跨 await 是有意的，与 `model_request` 那条同形（clippy 的 await_holding_lock 会误报）。
#[allow(clippy::await_holding_lock)]
async fn a_session_command_stays_hermetic_under_an_outer_cargo_target_dir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let f = fixture();
    let polluted = "/tmp/gate-sim/shared-target";
    let _pollution = OuterTargetDir::install(polluted);

    // 值班长命令（`session_id = Some`）：执行器**不**注入，子进程只会原样继承外层变量。
    let mut ctx = f.ctx.clone();
    ctx.session_id = Some("sess1".into());
    let out = f
        .executor
        .execute(&call("echo v=$CARGO_TARGET_DIR"), &ctx)
        .await
        .unwrap();

    // 子命令确实看见了外层污染（这也正是旧子串断言会假红的机制）。
    assert!(
        out.content.contains(polluted),
        "值班长命令应原样继承外层 `CARGO_TARGET_DIR`：{}",
        out.content
    );
    assert!(
        out.content.contains("shared-target"),
        "污染串含 `shared-target` 子串——旧写法会被它误伤：{}",
        out.content
    );

    // hermetic 断言：不得含**执行器自己 home 的**共享路径 → 外层污染下照样成立。
    let injected = f.home.shared_target_path().display().to_string();
    assert!(
        !out.content.contains(&injected),
        "hermetic：值班长命令不得拿到执行器的共享路径（外层污染不得掩盖注入回归）：{}",
        out.content
    );
}

/// 正向对照：任务命令（`session_id = None`）确实拿到**执行器自己 home 的**共享路径——
/// 即使外层已被污染（子进程环境里同名键由执行器注入覆盖）。
#[tokio::test]
// 同上：进程级环境，跨 await 持锁是有意的。
#[allow(clippy::await_holding_lock)]
async fn a_task_command_gets_the_executors_own_shared_path_even_when_the_outer_env_is_polluted() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let f = fixture();
    let _pollution = OuterTargetDir::install("/tmp/gate-sim/shared-target");

    let out = f
        .executor
        .execute(&call("echo v=$CARGO_TARGET_DIR"), &f.ctx)
        .await
        .unwrap();

    let injected = f.home.shared_target_path().display().to_string();
    assert!(
        out.content.contains(&injected),
        "任务命令应拿到执行器自己 home 的共享路径 `{injected}`：{}",
        out.content
    );
}

// ─────────────────── 场景 7：新注释不引入 rustdoc 层面的告警（AC-1 / AC-4）───────────────────

/// 被点名的用例用反引号包裹（普通代码片段），**不是** `[`…`]` intra-doc link。
#[test]
fn the_lesson_reference_is_a_code_span_not_an_intra_doc_link() {
    let lesson = lesson_section(&runner_for_doc_block(TOOLS_SRC));
    assert!(
        lesson.contains(&format!("`{REFERENCED_CASE}`")),
        "引用应是反引号代码片段：{lesson}"
    );
    assert!(
        !lesson.contains(&format!("[`{REFERENCED_CASE}`]")),
        "引用不得写成 `[`…`]` 形式的 intra-doc link（会引入 rustdoc 悬空链接告警）：{lesson}"
    );
}
