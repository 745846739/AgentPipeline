//! 黄金剧本：真 LLM 人工验收（决策 261 / 票 foreman-operate-pipeline 04，`#[ignore]`，
//! **不进任何自动门**）。
//!
//! 剧本原文（三类场景、期望、反例）在 `.scratch/foreman-operate-pipeline/issues/01-decision-and-handbook.md`
//! 的「黄金剧本（草案，票 04 执行）」一节；本文件是它的可执行装置——与 `llm_smoke.rs`
//! 同一副环境变量与姿态（`#[ignore]` + 环境变量双锁）：
//!
//! ```text
//! AGENTPIPELINE_SMOKE_VENDOR=openai \
//! AGENTPIPELINE_SMOKE_MODEL=gpt-4o-mini \
//! AGENTPIPELINE_SMOKE_API_KEY=sk-... \
//! AGENTPIPELINE_SMOKE_BASE_URL=http://127.0.0.1:8787/v1 \
//! cargo test -p agentpipeline-core --test integration golden_script -- --ignored --nocapture
//! ```
//!
//! **断言的边界**（spec 口径：不测 prompt 的效果进自动门）：结构性期望进门——几张卡、
//! 参数是不是从此刻 `allowed_actions` 来的、读问题零提议、停因原文逐字在不在、写轮真拉了
//! 手册；**措辞质量不进门**——回话与提议逐条 `println`，由值班经理人肉确认后把记录贴回票 04。

use std::sync::Arc;

use agentpipeline_core::actions::allowed_actions;
use agentpipeline_core::agent::client::LlmClient;
use agentpipeline_core::agent::factory;
use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::ForemanRunner;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{PendingKind, PendingReason, Provider, TaskStatus};
use testkit::{SseRecorder, TestHome};

fn smoke_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// 与 `foreman.rs` 的 `park_task` 同一姿势：游标 pending + 投影，两步都走。
async fn park(store: &Store, task_id: &str, kind: PendingKind, message: &str) {
    let cursors = store.load_live_cursors(task_id).await.unwrap();
    let cursor = cursors
        .first()
        .expect("seed_task 应当建了 main 游标")
        .clone();
    let reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
    store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    store.sync_task_projection(task_id).await.unwrap();
}

fn print_turn(
    tag: &str,
    instruction: &str,
    traces: &[agentpipeline_core::pipeline::foreman::ForemanTrace],
    reply: &str,
) {
    println!("\n========== {tag} ==========");
    println!("指令：{instruction}");
    let seq: Vec<String> = traces
        .iter()
        .map(|t| format!("{}{}", t.tool, if t.ok { "" } else { "(失败)" }))
        .collect();
    println!(
        "工具痕迹：{}（若含 Skill 即按点名拉了手册）",
        seq.join(" → ")
    );
    println!("回话：{reply}");
}

#[tokio::test]
#[ignore = "需要真实 LLM key；黄金剧本人工验收，用法见文件头注释"]
async fn golden_script_three_scenarios_with_a_real_llm() {
    let Some(api_key) = smoke_env("AGENTPIPELINE_SMOKE_API_KEY") else {
        eprintln!("未设置 AGENTPIPELINE_SMOKE_API_KEY，跳过黄金剧本");
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
            vendor,
            model,
            context_window: 128_000,
            base_url: smoke_env("AGENTPIPELINE_SMOKE_BASE_URL"),
            api_key: Some(api_key),
            enabled: true,
            created_at: store.now(),
            updated_at: store.now(),
        })
        .await
        .unwrap();

    // 与生产启动同一个播种函数（决策 261）：点名与手册都就位才开演
    let seeded = factory::seed_factory_defaults(home.home(), &store)
        .await
        .unwrap();
    assert!(seeded.pointer_written, "foreman 行应当播下点名：{seeded:?}");
    assert!(
        seeded
            .skills_written
            .iter()
            .any(|s| s == "operate-pipeline"),
        "手册应当本次种入：{seeded:?}"
    );

    // ── 剧本前置：三类场景各一组任务 ──
    let repo = home.scratch_dir("proj");
    testkit::seed_project(&store, "p1", "黄金剧本", &repo, "main")
        .await
        .unwrap();
    // A：pending(重试耗尽)，可推进
    testkit::seed_task(&store, "t1", "p1").await.unwrap();
    park(
        &store,
        "t1",
        PendingKind::RetryExhausted,
        "重试耗尽，等你拍板",
    )
    .await;
    // B：三张终态
    for id in ["t2", "t3", "t4"] {
        testkit::seed_task(&store, id, "p1").await.unwrap();
        store.set_task_status(id, TaskStatus::Failed).await.unwrap();
    }
    // C：停因原文可引用
    testkit::seed_task(&store, "t5", "p1").await.unwrap();
    park(
        &store,
        "t5",
        PendingKind::UserDecision,
        "冲突了两条路，你挑一条",
    )
    .await;

    let sse = Arc::new(SseRecorder::new());
    let llm: Arc<dyn LlmClient> = Arc::new(ProductionLlm::new(store.clone(), sse.clone()));
    let runner = ForemanRunner::new(
        store.clone(),
        Settings::default(),
        home.home().clone(),
        llm,
        sse.clone(),
    );

    // ── A 类 · 单票推进 ──
    let sid_a = store
        .create_foreman_session("黄金剧本 A · 单票推进")
        .await
        .unwrap()
        .id;
    let turn_a = runner
        .say(Some(&sid_a), "把 t1 推进一步")
        .await
        .expect("A 类回话应当成功");
    let pa = store.list_pending_foreman_proposals(&sid_a).await.unwrap();
    print_turn(
        "A 类 · 单票推进",
        "把 t1 推进一步",
        &turn_a.traces,
        &turn_a.reply,
    );
    for p in &pa {
        println!(
            "提议：tool={} args={} summary={}",
            p.tool, p.args, p.summary
        );
    }

    // 结构期望（A）
    assert!(
        turn_a.traces.iter().any(|t| t.tool == "Skill" && t.ok),
        "写轮应先按点名拉手册：{:?}",
        turn_a.traces
    );
    assert_eq!(pa.len(), 1, "A 类应当恰好一张提议：{pa:?}");
    assert_eq!(pa[0].tool, "task");
    assert_eq!(pa[0].args["task_id"], "t1");
    assert_eq!(
        pa[0].args["action"], "resume",
        "推进 = resume 类：{}",
        pa[0].args
    );
    let resume_action = pa[0].args["resume_action"].as_str().unwrap_or("");
    let t1 = store.get_task("t1").await.unwrap();
    let reason = t1.pending_reason.clone().expect("t1 应当停在 pending");
    let allowed = allowed_actions(&reason, None);
    assert!(
        allowed.iter().any(|a| a.action == resume_action),
        "resume_action 必须来自此刻的 allowed_actions：{resume_action} vs {:?}",
        allowed
            .iter()
            .map(|a| a.action.as_str())
            .collect::<Vec<_>>()
    );
    if resume_action == "goto" {
        // goto 的按钮参数带落点（actionSubmit.ts 同源）——缺了它按键必 400
        assert!(
            pa[0]
                .args
                .get("target_stage")
                .and_then(|v| v.as_str())
                .is_some()
                && pa[0]
                    .args
                    .get("target_node")
                    .and_then(|v| v.as_str())
                    .is_some(),
            "goto 要带落点：{}",
            pa[0].args
        );
    }
    assert_eq!(
        store.get_task("t1").await.unwrap().status,
        TaskStatus::Pending,
        "提议不是执行：按键前任务不动"
    );

    // ── B 类 · 批量重试 ──
    let sid_b = store
        .create_foreman_session("黄金剧本 B · 批量重试")
        .await
        .unwrap()
        .id;
    let turn_b = runner
        .say(Some(&sid_b), "这三张挨个重试")
        .await
        .expect("B 类回话应当成功");
    let pb = store.list_pending_foreman_proposals(&sid_b).await.unwrap();
    print_turn(
        "B 类 · 批量重试",
        "这三张挨个重试",
        &turn_b.traces,
        &turn_b.reply,
    );
    for p in &pb {
        println!(
            "提议：tool={} args={} summary={}",
            p.tool, p.args, p.summary
        );
    }

    // 结构期望（B）：逐票一卡、跨任务并存、每任务至多一张、参数是 retry
    assert!(
        turn_b.traces.iter().any(|t| t.tool == "Skill" && t.ok),
        "写轮应先按点名拉手册：{:?}",
        turn_b.traces
    );
    assert_eq!(pb.len(), 3, "B 类应当逐票三张：{pb:?}");
    let mut targets: Vec<String> = pb
        .iter()
        .map(|p| p.args["task_id"].as_str().unwrap_or("").to_string())
        .collect();
    targets.sort();
    assert_eq!(targets, ["t2", "t3", "t4"], "每任务恰好一张、三张齐");
    for p in &pb {
        assert_eq!(p.tool, "task");
        assert_eq!(p.args["action"], "retry", "{}", p.args);
    }
    for id in ["t2", "t3", "t4"] {
        assert_eq!(
            store.get_task(id).await.unwrap().status,
            TaskStatus::Failed,
            "{id}：提议不是执行，按键前终态不动"
        );
    }

    // ── C 类 · 只读问答 ──
    let sid_c = store
        .create_foreman_session("黄金剧本 C · 只读问答")
        .await
        .unwrap()
        .id;
    let turn_c = runner
        .say(Some(&sid_c), "t5 为什么停了？")
        .await
        .expect("C 类回话应当成功");
    let pc = store.list_pending_foreman_proposals(&sid_c).await.unwrap();
    print_turn(
        "C 类 · 只读问答",
        "t5 为什么停了？",
        &turn_c.traces,
        &turn_c.reply,
    );

    // 结构期望（C）：零提议 + 停因原文逐字引用
    assert!(pc.is_empty(), "读类指令零提议：{pc:?}");
    assert!(
        turn_c.reply.contains("冲突了两条路，你挑一条"),
        "停因必须引用台账原文（不转述枚举名）：{}",
        turn_c.reply
    );

    println!(
        "\n三类场景结构断言全过；token：A {}/{} · B {}/{} · C {}/{}。措辞质量由人确认后贴回票 04。",
        turn_a.prompt_tokens,
        turn_a.completion_tokens,
        turn_b.prompt_tokens,
        turn_b.completion_tokens,
        turn_c.prompt_tokens,
        turn_c.completion_tokens
    );
}
