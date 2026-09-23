//! 真 LLM 冒烟测试（testing.md §3.2，`#[ignore]`，不进任何自动门）。
//!
//! 手动运行（需要真实 key）：
//!
//! ```text
//! AGENTPIPELINE_SMOKE_VENDOR=openai \
//! AGENTPIPELINE_SMOKE_MODEL=gpt-4o-mini \
//! AGENTPIPELINE_SMOKE_API_KEY=sk-... \
//! AGENTPIPELINE_SMOKE_BASE_URL=https://api.openai.com/v1 \  # 可省略，按 vendor 取官方默认
//! cargo test -p agentpipeline-core --test llm_smoke -- --ignored --nocapture
//! ```
//!
//! **两条冒烟：**
//! - [`real_llm_completes_architect_execute_with_structured_metadata`]（单节点，票 13）：
//!   适配器真调用 + 流式增量 + token 计量 + `submit_metadata` 结构化输出可解析；
//! - [`real_llm_drives_full_flow_to_merge_approval`]（全流程，主流程票 04）：真模型驱动
//!   完整主流程到 `pending(merge_approval)`，闸门在真模型下真跑（Node 工程 fixture）。
//!   真模型触发的 `UserDecision` 由 [`auto_answer_pending`] 自动应答（轮换 goto 候选），
//!   其余 pending 一律失败并打印可定位诊断。
//!
//! **已知边界（票面要求写明，不得静默）：** 真模型输出不确定，本冒烟是**人工确认手段**
//! 而非回归门——它绿不保证明天绿；它的价值是把「真模型路径完全无人知晓」变成
//! 「发版前有人跑过」。实测环境与结论见 `.scratch/agentpipeline-mainflow-e2e/issues/04-real-llm-full-flow-smoke.md`。

use std::sync::Arc;

use agentpipeline_core::agent::client::{submit_metadata_tool, LlmClient, LlmRequest, RunContext};
use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::sse::SseEventType;
use agentpipeline_core::types::{ArchitectExecuteMetadata, Node, Provider, Stage};
use agentpipeline_core::Error;
use testkit::{SseRecorder, TestHome};

fn smoke_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
#[ignore = "需要真实 LLM key；用法见文件头注释"]
async fn real_llm_completes_architect_execute_with_structured_metadata() {
    let Some(api_key) = smoke_env("AGENTPIPELINE_SMOKE_API_KEY") else {
        eprintln!("未设置 AGENTPIPELINE_SMOKE_API_KEY，跳过真 LLM 冒烟");
        return;
    };
    let vendor = smoke_env("AGENTPIPELINE_SMOKE_VENDOR").unwrap_or_else(|| "openai".into());
    let model = smoke_env("AGENTPIPELINE_SMOKE_MODEL").unwrap_or_else(|| match vendor.as_str() {
        "anthropic" => "claude-sonnet-4-5".into(),
        "deepseek" => "deepseek-chat".into(),
        _ => "gpt-4o-mini".into(),
    });

    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    store
        .upsert_provider(&Provider {
            id: "smoke".into(),
            vendor: vendor.clone(),
            model: model.clone(),
            context_window: 128_000,
            base_url: smoke_env("AGENTPIPELINE_SMOKE_BASE_URL"),
            api_key: Some(api_key),
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));

    // architect-design.execute 的一次最小真调用：要求提交结构化元数据
    let request = LlmRequest {
        stage: Stage::ArchitectDesign,
        node: Node::Execute,
        attempt: 1,
        system_prompt: "你是架构师。只提交元数据，不要写文件。".into(),
        user_prompt: "为「任务目录切换为按创建时间排序」提交架构元数据（readiness=true）。".into(),
        messages: vec![],
        tools: vec![submit_metadata_tool::<ArchitectExecuteMetadata>(
            "提交架构设计元数据",
        )],
        temperature: None,
        max_tokens: None,
        provider_id: None,
        run: Some(RunContext {
            task_id: "smoke".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
            session_id: String::new(),
        }),
    };
    let response = client.complete(request).await.unwrap();

    // 计量与流式
    println!(
        "vendor={vendor} model={model} prompt={} completion={}",
        response.prompt_tokens, response.completion_tokens
    );
    assert!(response.prompt_tokens > 0, "prompt tokens 应有计量");
    assert!(
        sse.count_of(SseEventType::ConversationDelta) > 0,
        "流式增量应有事件"
    );

    // 结构化输出：submit_metadata 参数可解析为阶段结构体（决策 38 链路）
    let call = response
        .tool_calls
        .iter()
        .find(|c| c.name == "submit_metadata")
        .ok_or_else(|| {
            Error::Validation(format!(
                "未提交 submit_metadata：tool_calls={:?} content={:?}",
                response.tool_calls, response.content
            ))
        })
        .unwrap();
    let metadata: ArchitectExecuteMetadata = serde_json::from_str(&call.arguments).unwrap();
    println!("metadata readiness={}", metadata.readiness);
}

// ─────────────────────────── 全流程真模型冒烟（主流程票 04）───────────────────────────
//
// 真 key + 真模型驱动**完整主流程**（init → architect-design → 并行设计 → sync-check →
// develop → review → test → merge 阶段 A），停在 `pending(merge_approval)`。
// 与上面的单节点冒烟互补：这里覆盖的是 mock 脚本永远给不了的「真模型的工具调用形态」
// ——JSON 包代码块、tool_call 分片、拒绝调工具、usage 缺省等。
//
// fixture 是**真实可构建的 Node 工程**（与前端 e2e 票 02 同源）：`npm test` 真跑，
// 闸门在真模型下也是真闸门。任务描述刻意收窄（实现 add 并通过 npm test），把真模型的
// 发挥空间压到最小，降低「冒烟因任务太难而挂」的噪声。
//
// 失败时打印：所有 run 行（stage/node/attempt/error）、命令记录（含退出码）、任务状态
// 与 pending 原因——足够定位是哪个 (stage, node) 的什么错误，而不是一个超时。

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::storage::tasks::NewTask;
use agentpipeline_core::types::{PendingKind, Project, TaskStatus};
use std::time::Duration;
use testkit::{RecordingKiller, Repo};

const FULL_FLOW_DEADLINE: Duration = Duration::from_secs(30 * 60);

#[tokio::test]
#[ignore = "需要真实 LLM key；用法见文件头注释（同单节点冒烟的环境变量）"]
async fn real_llm_drives_full_flow_to_merge_approval() {
    let Some(api_key) = smoke_env("AGENTPIPELINE_SMOKE_API_KEY") else {
        eprintln!("未设置 AGENTPIPELINE_SMOKE_API_KEY，跳过真 LLM 全流程冒烟");
        return;
    };
    let vendor = smoke_env("AGENTPIPELINE_SMOKE_VENDOR").unwrap_or_else(|| "openai".into());
    let model = smoke_env("AGENTPIPELINE_SMOKE_MODEL").unwrap_or_else(|| match vendor.as_str() {
        "anthropic" => "claude-sonnet-4-5".into(),
        "deepseek" => "deepseek-chat".into(),
        _ => "gpt-4o-mini".into(),
    });
    let started = std::time::Instant::now();

    // ── 装置：临时 home + 真实 Node 工程 fixture（闸门真跑）──
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = Repo::clean().unwrap();
    make_node_fixture(&repo);

    store
        .upsert_provider(&Provider {
            id: "smoke".into(),
            vendor: vendor.clone(),
            model: model.clone(),
            context_window: 128_000,
            base_url: smoke_env("AGENTPIPELINE_SMOKE_BASE_URL"),
            api_key: Some(api_key),
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let project = Project {
        id: "p1".into(),
        name: "真模型冒烟".into(),
        local_path: repo.path().display().to_string(),
        default_branch: "main".into(),
        language: Some("node".into()),
        test_framework: Some("npm test --silent".into()),
        lint_command: None,
        agents_md_path: None,
        created_at: store.now(),
    };
    store.create_project(&project).await.unwrap();

    let mut new_task = NewTask::new("t1", "真模型冒烟：实现 add", "p1");
    new_task.description = "在现有 Node 工程里实现 src/lib.js 的 add(a, b)（返回 a + b），\
        使 `npm test` 通过。不要修改 package.json、run-tests.js 与 tests/ 目录。\
        完成后用 git 提交（用户名/邮箱任意）。"
        .into();
    store.create_task(&new_task).await.unwrap();

    let sse = SseRecorder::new();
    let killer = RecordingKiller::new();
    let llm = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));
    let settings = Settings::default();
    let executor = Arc::new(Executor::new(
        store.clone(),
        settings.clone(),
        Arc::new(sse.clone()),
        Arc::new(llm),
        Arc::new(killer.clone()),
    ));
    let scheduler = KanbanScheduler::new(
        store.clone(),
        settings,
        Arc::new(SystemClock),
        Arc::new(killer),
        Arc::new(sse.clone()),
        Arc::new(|_t: &str| {}),
    );
    scheduler.tick().await.unwrap();

    // ── 驱动到 merge 阶段 A（executor.run 在 pending 处返回；真模型耗时可能数十分钟）──
    //
    // 真模型可能触发需要用户拍板的 UserDecision（评审不合格、信息不足等）——这是
    // 正常主流程行为，不是冒烟失败。冒烟对它**自动应答**（优先 continue，其次 skip，
    // 每次留痕），把流程推进到终点；其余 pending（retry_exhausted / merge_approval
    // 之外的异常）原样失败并打印诊断。上限 8 次，防真模型在坏循环里空转。
    let mut auto_answers: u32 = 0;
    let pending = loop {
        let run = tokio::time::timeout(FULL_FLOW_DEADLINE, executor.run("t1")).await;
        let task = store.get_task("t1").await.unwrap();
        let pending = store
            .load_live_cursors("t1")
            .await
            .unwrap()
            .into_iter()
            .find_map(|c| c.pending_reason);
        if run.is_err() || task.status != TaskStatus::Pending || pending.is_none() {
            dump_diagnostics(&store, "t1").await;
            panic!(
                "全流程冒烟未到达 merge_approval：status={:?} pending={pending:?} 耗时={:?}",
                task.status,
                started.elapsed()
            );
        }
        let pending = pending.unwrap();
        match pending.kind {
            PendingKind::MergeApproval => break pending,
            PendingKind::UserDecision if auto_answers < 8 => {
                auto_answers += 1;
                auto_answer_pending(&store, "t1", &pending, auto_answers - 1).await;
            }
            _ => {
                dump_diagnostics(&store, "t1").await;
                panic!("停点 {pending:?} 不是 merge_approval（其余 pending 都算失败，诊断已打印）");
            }
        }
    };
    let _ = pending;
    eprintln!(
        "冒烟途中自动应答了 {auto_answers} 次用户决策（见上方留痕）——真模型确实触发了人工拍板点"
    );

    // ── 断言关键接缝 ──
    // ① 12 个 agent 节点都有 run 行（每个阶段都被真模型真实走过）
    let runs = store.list_runs("t1").await.unwrap();
    let mut seen: Vec<(Stage, Node)> = runs.iter().map(|r| (r.stage, r.node)).collect();
    seen.sort();
    seen.dedup();
    for (stage, node) in EXPECTED_AGENT_NODES {
        assert!(
            seen.contains(&(stage, node)),
            "缺 {stage}.{node} 的 run 行；实际 = {seen:?}"
        );
    }
    // ② 真模型返回格式下 submit_metadata 被正确解析：架构产出以 design_doc 落
    //    stage output（决策 115 允许 agent 自选文件名，登记路径才是权威，不假设 design.md）
    let outputs = store.list_stage_outputs("t1").await.unwrap();
    let design_doc = outputs
        .iter()
        .find(|o| o.stage == Stage::ArchitectDesign && o.output_type == "design_doc")
        .unwrap_or_else(|| {
            panic!(
                "architect.execute 应登记 design_doc stage output；实际 = {:?}",
                outputs
                    .iter()
                    .map(|o| (o.stage, o.output_type.as_str(), o.file_path.as_str()))
                    .collect::<Vec<_>>()
            )
        });
    // 登记路径可能是绝对路径，也可能是相对路径（真模型两种都可能给）；两者都接受，
    // 但**必须真实可读**——登记了一个不存在的文件才是缺陷。
    let registered = std::path::PathBuf::from(&design_doc.file_path);
    let candidates = if registered.is_absolute() {
        vec![registered.clone()]
    } else {
        vec![
            home.home().task_file("t1", &design_doc.file_path),
            home.home().worktree_path("t1").join(&design_doc.file_path),
        ]
    };
    let found = candidates.iter().find(|p| p.exists());
    if found.is_none() {
        // 诊断：把两个根下实际有啥打出来，避免只报一句「不存在」
        let mut listing = Vec::new();
        for root in [home.home().task_dir("t1"), home.home().worktree_path("t1")] {
            if let Ok(entries) = std::fs::read_dir(&root) {
                for e in entries.flatten() {
                    listing.push(format!(
                        "{}/{}",
                        root.display(),
                        e.file_name().to_string_lossy()
                    ));
                }
            }
        }
        panic!(
            "登记的设计文档应真实存在；registered={:?} 候选={:?}\n实际文件：{:#?}",
            design_doc.file_path, candidates, listing
        );
    }
    // ③ token 计量 > 0（真模型 usage 字段命名差异不许静默归零）
    let task = store.get_task("t1").await.unwrap();
    assert!(task.total_tokens > 0, "token 计量应 > 0");
    assert!(task.total_calls > 0, "调用计数应 > 0");
    // ④ 闸门真跑：npm test 以退出码 0 被记录（真模型写的代码真的过了测试）
    let commands = store.list_commands("t1", None, None).await.unwrap();
    assert!(
        commands
            .iter()
            .any(|c| c.command.contains("npm test") && c.exit_code == Some(0)),
        "应有退出码 0 的 npm test 记录；命令 = {:?}",
        commands
            .iter()
            .map(|c| (&c.command, c.exit_code))
            .collect::<Vec<_>>()
    );

    println!(
        "✓ 真模型全流程冒烟通过：vendor={vendor} model={model} 耗时={:?} tokens={} calls={} runs={}",
        started.elapsed(),
        task.total_tokens,
        task.total_calls,
        runs.len()
    );
}

/// 12 个 agent 节点（§10.3；与 FakeAgent 场景同一份清单）。
const EXPECTED_AGENT_NODES: [(Stage, Node); 12] = [
    (Stage::ArchitectDesign, Node::ValidateInput),
    (Stage::ArchitectDesign, Node::Execute),
    (Stage::ArchitectDesign, Node::ValidateOutput),
    (Stage::DevelopDesign, Node::ValidateInput),
    (Stage::DevelopDesign, Node::Execute),
    (Stage::DevelopDesign, Node::ValidateOutput),
    (Stage::TestDesign, Node::ValidateInput),
    (Stage::TestDesign, Node::Execute),
    (Stage::TestDesign, Node::ValidateOutput),
    (Stage::Develop, Node::Execute),
    (Stage::Review, Node::Execute),
    (Stage::Test, Node::Execute),
];

/// 把 testkit 的 Rust fixture 覆写成真实可构建的 Node 工程（票 02 同源）：
/// `npm test` 失败于「未实现」占位，真模型写完 `add` 后闸门才转绿。
fn make_node_fixture(repo: &Repo) {
    repo.write(
        "package.json",
        "{\n  \"name\": \"smoke-fixture\",\n  \"private\": true,\n  \"scripts\": {\n    \"test\": \"node run-tests.js\"\n  }\n}\n",
    );
    repo.write(
        "run-tests.js",
        "const { add } = require('./src/lib.js');\nif (add(1, 2) !== 3) {\n  console.error('FAIL: add(1,2) !== 3');\n  process.exit(1);\n}\nconsole.log('PASS');\n",
    );
    repo.write(
        "src/lib.js",
        "function add(a, b) { throw new Error('not implemented'); }\nmodule.exports = { add };\n",
    );
    repo.write(
        "tests/acceptance.js",
        "const { add } = require('../src/lib.js');\nif (add(1, 2) !== 3) throw new Error('acceptance: add(1,2) !== 3');\n",
    );
    repo.git(&[
        "rm",
        "-q",
        "Cargo.toml",
        "src/lib.rs",
        "tests/acceptance.rs",
    ]);
    repo.git(&["add", "-A"]);
    repo.git(&[
        "-c",
        "user.name=smoke",
        "-c",
        "user.email=smoke@localhost",
        "commit",
        "-m",
        "chore: 切换为 Node 工程 fixture",
    ]);
}

/// 失败诊断：run 行 / 命令记录 / pending 原因 / 对话最后几条——够定位到 (stage, node)。
async fn dump_diagnostics(store: &agentpipeline_core::storage::Store, task_id: &str) {
    eprintln!("────── 真模型冒烟诊断 ──────");
    match store.list_runs(task_id).await {
        Ok(runs) => {
            eprintln!("run 行（{}）：", runs.len());
            for r in &runs {
                eprintln!(
                    "  {:?}.{:?} attempt={} status={:?} tokens={}/{} err={:?}",
                    r.stage,
                    r.node,
                    r.attempt,
                    r.status,
                    r.prompt_tokens,
                    r.completion_tokens,
                    r.error
                );
            }
        }
        Err(e) => eprintln!("list_runs 失败：{e}"),
    }
    match store.list_commands(task_id, None, None).await {
        Ok(cmds) => {
            eprintln!("命令记录（{}）：", cmds.len());
            for c in &cmds {
                eprintln!(
                    "  {:?}.{:?} `{}` exit={:?}",
                    c.stage, c.node, c.command, c.exit_code
                );
                // 失败命令必须带输出：闸门失败的真实原因（测试断言、lint 报错）只在这里，
                // 只看 exit code 会把「为什么失败」丢掉（票 04 首轮实测）。
                if c.exit_code.is_some_and(|code| code != 0) {
                    for (label, preview) in
                        [("stdout", &c.stdout_preview), ("stderr", &c.stderr_preview)]
                    {
                        if let Some(p) = preview {
                            let tail: String = p
                                .chars()
                                .rev()
                                .take(1200)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect();
                            eprintln!("      [{label} 尾部]\n{tail}");
                        }
                    }
                }
            }
        }
        Err(e) => eprintln!("list_commands 失败：{e}"),
    }
    // 阶段产出元数据：闸门失败详情（gate_failure_output / blockers）落在这里
    if let Ok(outputs) = store.list_stage_outputs(task_id).await {
        for o in outputs {
            let meta = o
                .metadata_json
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_default();
            if meta.contains("gate_failure") || meta.contains("blocker") {
                eprintln!(
                    "阶段产出 {:?}/{} metadata：\n{}",
                    o.stage,
                    o.output_type,
                    meta.chars().take(1500).collect::<String>()
                );
            }
        }
    }
    match store.load_live_cursors(task_id).await {
        Ok(cursors) => {
            for c in cursors {
                eprintln!(
                    "游标 {:?}.{:?} pending={:?}",
                    c.stage, c.node, c.pending_reason
                );
            }
        }
        Err(e) => eprintln!("load_live_cursors 失败：{e}"),
    }
    eprintln!("────────────────────────────");
}

/// UserDecision 的冒烟自动应答：优先 `continue`（裁决合格/继续），其次 `skip`
/// （强制进入下一阶段），再次 goto（按 `rotation` **轮换**多个候选）。
///
/// 每次都留痕——冒烟绿不代表模型没要求人工介入，只代表介入点不阻断流程。
/// 需要自由输入或没有出路的 pending 不在自动应答范围，交由上层失败诊断。
///
/// **为什么要轮换而不是固定取第一个：** `gate_recheck` 同时给两条出路
/// （「修改测试用例」= test.execute、「修改业务代码」= develop.execute）。固定取
/// 第一条时，若真实失败其实在业务代码，每次只是重跑 test.execute，问题原地不动、无限
/// 循环到上限（票 04 实测：8 次全打在 test.execute，任务卡在 gate_recheck 出不来）。
/// 轮换让两条出路各获得机会，这才是「自动应答」应有的探索行为。
async fn auto_answer_pending(
    store: &agentpipeline_core::storage::Store,
    task_id: &str,
    pending: &agentpipeline_core::types::PendingReason,
    rotation: u32,
) {
    let acts = store.allowed_actions_for_task(task_id).await.unwrap();
    // 出路优先级：continue（裁决合格）> skip（强制放行）> goto 候选**轮换**
    // （同阶段 goto 与跨阶段 goto 合并成候选集，按 rotation 取模选一条）。
    let mut goto_candidates: Vec<&agentpipeline_core::actions::AllowedAction> =
        acts.iter().filter(|a| a.action == "goto").collect();
    // 稳定排序（stage 名 + node 名）保证轮换顺序可复现
    goto_candidates.sort_by_key(|a| {
        a.target
            .as_ref()
            .map(|t| (t.stage.as_str().to_string(), t.node.as_str().to_string()))
    });
    let rotated_goto: Option<&agentpipeline_core::actions::AllowedAction> =
        if goto_candidates.is_empty() {
            None
        } else {
            Some(goto_candidates[(rotation as usize) % goto_candidates.len()])
        };
    let choice = acts
        .iter()
        .find(|a| a.action == "continue")
        .or_else(|| acts.iter().find(|a| a.action == "skip"))
        .or(rotated_goto)
        .cloned();
    let Some(choice) = choice else {
        eprintln!(
            "自动应答：{:?} 在 {} 没有无输入动作（{:?}），留给上层失败诊断",
            pending.kind,
            pending.stage,
            acts.iter().map(|a| a.action.as_str()).collect::<Vec<_>>()
        );
        // 通过清除再触发失败路径：直接 panic 会让诊断更直接
        panic!("自动应答缺可用动作：pending = {pending:?}");
    };
    let cursor = store
        .resolve_sole_cursor(task_id)
        .await
        .unwrap()
        .expect("UserDecision 必有游标");
    let action = ResumeAction::parse(&choice.action).unwrap();
    let target = choice.target.as_ref().map(|t| (t.stage, t.node));
    eprintln!(
        "自动应答 #{:?}：{} 的 {}（{}）target={target:?}",
        pending.kind, pending.stage, choice.action, choice.label
    );
    agentpipeline_core::pipeline::resume::apply_action(
        store,
        &cursor,
        action,
        target,
        Some("（真模型冒烟自动应答）"),
    )
    .await
    .unwrap();
}
