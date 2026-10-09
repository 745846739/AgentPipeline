//! L2 集成（测试场景 4 / 场景 5 · AC-2）：**落库渲染字段与 dossier 载荷摘内部编号**。
//!
//! 文案纪律任务（决策 199 扩面）把「直呈给用户的落库报文」里的 `（决策 N）（票 N）`
//! 括注全部摘掉，同时保住人话语义。本文件钉三面：
//!
//! 1. **孤儿 run 的终态 `error`**（场景 4 步骤 1）：进程重启收口路径落库的字段——
//!    行为面直接跑 `abandon_stale_project_runs` / `abandon_stale_task_runs` 再回读。
//! 2. **改后字符串字面量的源面**（场景 4 步骤 3）：`observability.rs` 的 `error` 字段
//!    与 `executor.rs` 的 `test_blockers` / `metadata_gaps` 类载荷构造——与前端门
//!    copy-discipline 规则 4 同口径的窄形态扫描（日志 / 断言 / 注释不在形态内，不误杀）。
//! 3. **场景 5 正面**：豁免面（tracing 日志、prompt 注入块、`#[test]` 断言消息）里的
//!    内部编号**仍然存在**——整库搜改把它们抹掉 = 违反决策 199「注释 / 文档 / 日志照旧」
//!    的边界，此面即红。
//!
//! `test_blockers` / `metadata_gaps` 载荷的**行为面**（真跑 sync-check 落库）在
//! `executor.rs::sync_gate_payloads_speak_plain_human_without_internal_refs`。

use agentpipeline_core::storage::observability::{NewProjectRun, NewRun};
use agentpipeline_core::types::{Node, NodeStatus, Stage};
use testkit::{seed_project, seed_task, TestHome};

// ─────────────────────────────── 判据（与前端门规则 4 同口径） ───────────────────────────────

/// 内部编号的形态：`决策 N` / `票 N`——含全角数字、「决策 130 / 137」连写（首段数字
/// 成立即算命中）与票据号的圈码后缀（`票 02②`）。与 `copy-discipline.test.ts` 规则 4
/// 的 `BACKEND_REF` 同一口径，防止两侧判据漂移。
///
/// `pub(crate)`：`executor.rs` 的同步载荷用例复用这一份（同一集成二进制内的兄弟模块）。
pub(crate) fn internal_ref(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let digit = |c: char| c.is_ascii_digit() || ('０'..='９').contains(&c);
    let space = |c: char| c.is_whitespace() || c == '\u{3000}';
    for i in 0..chars.len() {
        let mark = match chars[i] {
            '决' if chars.get(i + 1) == Some(&'策') => 2,
            '票' => 1,
            _ => 0,
        };
        if mark == 0 {
            continue;
        }
        let mut j = i + mark;
        while j < chars.len() && space(chars[j]) {
            j += 1;
        }
        if j < chars.len() && digit(chars[j]) {
            let mut k = j;
            while k < chars.len()
                && (digit(chars[k]) || ('\u{2460}'..='\u{2473}').contains(&chars[k]))
            {
                k += 1;
            }
            return Some(chars[i..k].iter().collect());
        }
    }
    None
}

fn assert_numberless(what: &str, text: &str) {
    if let Some(found) = internal_ref(text) {
        panic!("{what}残留内部编号「{found}」：{text}");
    }
}

// ─────────────────────────────── 源面扫描的提取器 ───────────────────────────────

/// 取 `"…"` 字面量的内容（要求文本以引号开头；到下一个引号为止——与规则 4 的
/// `"([^"]*)"` 同口径，转义不另处理）。
fn quoted(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// 截一个 ≤`max` 字节的前缀，且落在字符边界上（字节窗口切在多字节汉字中间会 panic）。
fn window_of(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 形态①（leading）：`<形状>(` 之后的第一段字面量——允许空白 / 换行与 `format!(` 包一层。
/// 针 `<needle>` 的每次出现提取一次；形状对不上（如变量实参）则跳过。
fn leading_literals(code: &str, needle: &str, after: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(at) = rest.find(needle) {
        let tail = &rest[at + needle.len()..];
        let tail = window_of(tail, 600).trim_start();
        if let Some(window) = tail.strip_prefix(after) {
            let t = window.trim_start();
            let t = match t.strip_prefix("format!") {
                Some(s) => s.trim_start(),
                None => t,
            };
            let t = match t.strip_prefix('(') {
                Some(s) => s.trim_start(),
                None => t,
            };
            if let Some(lit) = quoted(t) {
                out.push(lit);
            }
        }
        rest = &rest[at + needle.len()..];
    }
    out
}

/// 形态③（first-quote）：调用窗内的**首个**字符串字面量（`insert_transition(…, Some("…"))`
/// 的实参在尾部，前导都是变量）。
fn first_quotes(code: &str, shape: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(at) = rest.find(shape) {
        let tail = &rest[at + shape.len()..];
        let tail = window_of(tail, 600);
        if let Some(start) = tail.find('"') {
            if let Some(lit) = quoted(&tail[start..]) {
                out.push(lit);
            }
        }
        rest = &rest[at + shape.len()..];
    }
    out
}

// ─────────────────── 场景 4 步骤 1：孤儿 run 落库的终态 error ───────────────────

#[tokio::test]
async fn restart_reaped_runs_land_plain_human_errors() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t-orph", "p1").await.unwrap();
    let cursor = store.load_live_cursors("t-orph").await.unwrap()[0].clone();

    // 「进程退出时还在跑」的两条孤儿行：项目级伪阶段 run + 任务级 run。
    store
        .insert_project_run(&NewProjectRun {
            project_id: "p1".into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "pseudo:project_analysis".into(),
        })
        .await
        .unwrap();
    let task_run = store
        .insert_run(&NewRun {
            task_id: "t-orph".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();

    // 启动恢复的收口路径（改后代码的对应路径——等价于重启后跑一次收口）。
    store.abandon_stale_project_runs().await.unwrap();
    let abandoned = store.abandon_stale_task_runs().await.unwrap();
    assert_eq!(abandoned.len(), 1, "任务级孤儿 run 应被收口");

    // ① 项目级孤儿 run 的终态 error（场景 4：不含 `（决策 212 / 票 13）`）
    let runs = store.list_project_runs("p1").await.unwrap();
    assert_eq!(runs[0].status, NodeStatus::Timeout, "{runs:?}");
    let error = runs[0].error.as_deref().unwrap_or_default();
    assert!(
        error.contains("进程重启") && error.contains("孤儿"),
        "人话语义保留（进程重启 + 为什么）：{error}"
    );
    assert_numberless("项目级孤儿 run 的 error", error);

    // ② 任务级遗留 run 的终态 error（场景 4：不含 `（票 02②）`）。
    // 返回行是**收口前**的快照，落库字段必须回读才算数。
    let rows = store.list_runs("t-orph").await.unwrap();
    let row = rows
        .iter()
        .find(|r| r.id == task_run)
        .expect("run 行收口后仍可查");
    assert_eq!(row.status, NodeStatus::Cancelled, "{row:?}");
    let error = row.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("进程重启"),
        "人话语义保留（说清是重启收口）：{error}"
    );
    assert_numberless("任务级遗留 run 的 error", error);
}

// ─────────────────── 场景 4 步骤 3：observability.rs 字符串字面量窄扫 ───────────────────

#[test]
fn observability_error_literals_carry_no_internal_refs() {
    // 扫描面：`error:` 字段构造（规则 4 形态①）+ 流转原因 + insert_transition 首字面量。
    // 日志 / 断言 / 注释不在形态内——observability 的注释里满是「决策 212 / 票 13」，
    // 那是决策 199 明文照旧的面，不许被本断言误杀。
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/storage/observability.rs"
    ));
    let mut checked = 0;
    for (needle, after) in [("error", ": Some("), ("reason", "= Some(")] {
        for lit in leading_literals(src, needle, after) {
            checked += 1;
            assert_numberless(&format!("observability.rs 的 {needle} 字段"), &lit);
        }
    }
    for lit in first_quotes(src, "insert_transition(") {
        checked += 1;
        assert_numberless("observability.rs 的 insert_transition 实参", &lit);
    }
    assert!(
        checked >= 2,
        "扫描面不能空转（形态没了要先重算这里）：实得 {checked}"
    );

    // 改后报文在场：摘编号 ≠ 改语义，两句人话一个字都不能少。
    assert!(
        src.contains("进程重启：项目级 run 成了孤儿，标终态"),
        "项目级孤儿收口报文应在场"
    );
    assert!(
        src.contains("进程重启：这一轮在上一进程退出时还在跑，标终态"),
        "任务级遗留收口报文应在场"
    );
}

// ─────────────────── 场景 4 步骤 3：executor.rs 载荷构造窄扫 ───────────────────

#[test]
fn executor_payload_literals_carry_no_internal_refs() {
    // 载荷 push 形态（规则 4 形态②，needle 留出 `.push(` 跨行的空隙）+ error 字段。
    // prompt 注入块（`# 未申报变更事实（确定性检查，决策 397）`）与 tracing 日志**不在**
    // 这些形态里——它们是场景 5 的豁免面，见下一条用例。
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/pipeline/executor.rs"
    ));
    let mut checked = 0;
    for needle in [
        "test_blockers",
        "dev_blockers",
        "metadata_gaps",
        "gaps",
        "warnings",
    ] {
        for lit in leading_literals(src, needle, ".push(") {
            checked += 1;
            assert_numberless(&format!("executor.rs 的 {needle} 载荷"), &lit);
        }
    }
    for lit in leading_literals(src, "error", ": Some(") {
        checked += 1;
        assert_numberless("executor.rs 的 error 字段", &lit);
    }
    for lit in first_quotes(src, "insert_transition(") {
        checked += 1;
        assert_numberless("executor.rs 的 insert_transition 实参", &lit);
    }
    assert!(
        checked >= 4,
        "扫描面不能空转（形态没了要先重算这里）：实得 {checked}"
    );

    // 改后报文在场：dossier 元数据卡要呈现的两句人话（摘 `（决策 136）`）。
    assert!(
        src.contains("high 场景「{name}」的 design_refs 缺失或悬空"),
        "悬空引用 blocker 报文应在场"
    );
    assert!(
        src.contains("architect-design 元数据缺 acceptance_criteria"),
        "metadata_gaps 点名 acceptance_criteria 的报文应在场"
    );
    assert!(
        src.contains("test-design 元数据缺 test_scenarios"),
        "metadata_gaps 点名 test_scenarios 的报文应在场"
    );
}

// ─────────────────── 场景 5 正面：豁免面的编号没有被抹掉 ───────────────────

#[test]
fn logs_prompts_and_assertions_keep_their_internal_refs() {
    // 决策 199 的边界：日志 / prompt 注入块 / 断言消息**照旧**带「决策 N」。
    // 整库搜改把它们抹掉 = 违反划界的误删，此条即红（场景 5「误删为失败」）。

    // ① tracing 日志面
    let merge = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/pipeline/merge.rs"
    ));
    assert!(
        merge.contains("基准已前移，approval 重置回阶段 A（决策 96）"),
        "tracing 日志面的内部编号被误删了"
    );

    // ② prompt 注入块（组装进模型请求的事实段标题）
    let executor = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/pipeline/executor.rs"
    ));
    assert!(
        executor.contains("# 未申报变更事实（确定性检查，决策 397）"),
        "prompt 注入块的内部编号被误删了"
    );

    // ③ `#[test]` 断言消息面
    let tools = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/agent/tools.rs"));
    assert!(
        tools.contains("卸载文件应真实落盘（决策 148）"),
        "断言消息的内部编号被误删了"
    );
}
