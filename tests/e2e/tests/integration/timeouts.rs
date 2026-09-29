//! E2E 超时与 resume 防连点（testing.md §8）：E2E-14 超时链、E2E-24 resume 防连点。
//!
//! 决策 33 / 36 / 64 / 66 / 100 / 122 / 320 / §3：假时钟驱动，手动 tick。

use std::time::Duration;

use super::common::Flow;
use agentpipeline_core::config::Settings;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::storage::observability::NewRun;
use agentpipeline_core::types::{Node, NodeStatus, PendingKind, PendingReason, Stage};
use testkit::{backdate_run, Script};

/// 轮询等待**那一条**卡死的 run 出现（executor 已进入第一个 LLM 调用）。
///
/// 按 `(stage, node)` 收窄到脚本挂了 `stall` 的那一个，而不是「该任务的第一条 running run」：
/// `init` 是**系统节点**（不调模型）而在它跑 git 活的这段时间里同样是 `running`，
/// 整套 gate 满负荷并行时轮询会先抓到它——随后它被判超时，又被它自己完成时按值写回
/// `success`（`finish_run` 的 status 是按值写的），于是「判超时」这条用例约 1/3 的概率
/// 红在一个与它无关的节点上。收窄之后这条等待才真的在等它要测的那一次调用。
async fn wait_for_active_run(flow: &Flow, task_id: &str) -> agentpipeline_core::types::NodeRun {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(run) = flow
                .store
                .active_runs()
                .await
                .unwrap()
                .into_iter()
                .find(|r| {
                    r.task_id.as_deref() == Some(task_id)
                        && r.stage == Stage::ArchitectDesign
                        && r.node == Node::ValidateInput
                })
            {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("executor 应进入第一个 LLM 调用")
}

// ─────────────────────────── E2E-14 ───────────────────────────

#[tokio::test]
async fn e2e_14_timeout_chain_kills_retries_then_pends_and_merge_has_no_skip() {
    let settings = Settings {
        node_idle_timeout_sec: 1,
        ..Default::default()
    };
    let f = Flow::with_settings(settings.clone()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .stall();
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t14", "p1").await.unwrap();
    f.admit("t14").await;

    // 真实 executor 进入 validate_input 后卡死（Stall），后台跑以便 tick 观察
    let ex = f.executor.clone();
    let handle = tokio::spawn(async move { ex.run("t14").await });
    let run = wait_for_active_run(&f, "t14").await;

    // 心跳停 → 空闲超时 → 杀进程组 → 连续第 1 次超时 → 自动续接拉起重试（决策 64 / 66 / 320）
    f.store.set_run_process_group(run.id, 4242).await.unwrap();
    backdate_run(&f.store, run.id, 400, 400).await.unwrap();
    let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs, vec![run.id]);
    assert_eq!(f.killer.killed_groups(), vec![4242], "杀整个进程组");
    assert!(
        report.timeout_pending_cursors.is_empty(),
        "连续第 1 次超时走续接支，不得 pending"
    );
    assert!(
        f.resumes.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "应拉起 executor 干净重试"
    );
    let finished = f
        .store
        .list_runs("t14")
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == run.id)
        .unwrap();
    assert_eq!(finished.status, NodeStatus::Timeout);
    handle.abort();

    // 决策 320 梯子：挂起看「该节点连续超时的轮数」，attempt 不再参与判断——
    // run1 已是连续第 1 次（上面那轮），第 2 次自动续接、第 3 次空白重跑，
    // 都不得挂起；连续第 4 次才 pending(timeout) 交回人工。
    let cursor = f.sole_cursor("t14").await;
    let mut resumes = f.resumes.load(std::sync::atomic::Ordering::SeqCst);
    for round in 2..=3u32 {
        let pg = 4241 + round as i32;
        let run_n = f
            .store
            .insert_run(&NewRun {
                task_id: "t14".into(),
                cursor_id: cursor.cursor_id.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt: round,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: Some(pg),
            })
            .await
            .unwrap();
        backdate_run(&f.store, run_n, 400, 400).await.unwrap();
        let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
        assert!(
            report.timeout_pending_cursors.is_empty(),
            "连续第 {round} 次超时走续接/空白重跑支，不得挂起"
        );
        assert!(
            f.killer.killed_groups().contains(&pg),
            "第 {round} 次超时照样杀进程组 {pg}"
        );
        let now = f.resumes.load(std::sync::atomic::Ordering::SeqCst);
        assert!(now > resumes, "第 {round} 次超时应拉起重试");
        resumes = now;
    }

    // 连续第 4 次：耗尽 → pending(timeout) 挂该游标
    let run4 = f
        .store
        .insert_run(&NewRun {
            task_id: "t14".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt: 4,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: Some(4245),
        })
        .await
        .unwrap();
    backdate_run(&f.store, run4, 400, 400).await.unwrap();
    let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
    assert_eq!(
        report.timeout_pending_cursors,
        vec![cursor.cursor_id.clone()],
        "连续第 4 次超时挂起交回人工（决策 320）"
    );
    let after = f.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        after.pending_reason.as_ref().unwrap().kind,
        PendingKind::Timeout
    );
    assert!(f.killer.killed_groups().contains(&4245));

    // merge 的 timeout 动作集无 skip（决策 122）
    testkit::seed_task(&f.store, "t14m", "p1").await.unwrap();
    let m = f.sole_cursor("t14m").await;
    f.store
        .set_cursor_pending(
            &m.cursor_id,
            &PendingReason::new(
                PendingKind::Timeout,
                Stage::Merge,
                Node::Execute,
                "合并超时",
            ),
        )
        .await
        .unwrap();
    let actions = f.store.allowed_actions_for_task("t14m").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert!(names.contains(&"goto") && names.contains(&"cancel"));
    assert!(
        !names.contains(&"skip"),
        "merge timeout 无 skip（决策 122）"
    );
}

#[tokio::test]
async fn e2e_14_long_system_command_heartbeat_survives_idle_timeout() {
    // 决策 100：600s 系统命令在 300s 空闲阈值下靠心跳存活
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t14h", "p1").await.unwrap();
    f.admit("t14h").await;
    let cursor = f.sole_cursor("t14h").await;
    let run = f
        .store
        .insert_run(&NewRun {
            task_id: "t14h".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: Some(5555),
        })
        .await
        .unwrap();
    // 已跑 600s，但 10s 前刚有活动
    backdate_run(&f.store, run, 600, 10).await.unwrap();
    let report = f.scheduler().tick().await.unwrap();
    assert!(
        report.timed_out_runs.is_empty(),
        "有心跳的长命令不得被空闲超时误杀"
    );
    assert!(!f.killer.was_called());
}

// ─────────────────────────── E2E-24 ───────────────────────────

#[tokio::test]
async fn e2e_24_resume_cooldown_detected_and_single_executor_guard() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t24", "p1").await.unwrap();
    f.admit("t24").await;

    // 造 pending 并 resume 一次：距上次 user_resume 0s < cooldown（app 据此 409 防连点）
    let cursor = f.sole_cursor("t24").await;
    f.store
        .set_cursor_pending(
            &cursor.cursor_id,
            &PendingReason::new(
                PendingKind::InfoInsufficient,
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "补充信息",
            ),
        )
        .await
        .unwrap();
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Continue,
        None,
        Some("补充"),
    )
    .await
    .unwrap();
    let elapsed = f
        .store
        .seconds_since_last_user_resume("t24")
        .await
        .unwrap()
        .expect("已记录 user_resume");
    assert!(
        elapsed < f.settings.pending_resume_cooldown_sec as i64,
        "冷却窗口内：elapsed={elapsed}"
    );

    // 单执行者守卫（决策 36）：第一次 run 卡住时，第二次 run 直接返回、不重复执行
    let mut stalling = Script::new();
    stalling
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .stall();
    f.agent.set_script(stalling);
    f.store
        .set_task_status("t24", agentpipeline_core::types::TaskStatus::Running)
        .await
        .unwrap();
    let ex = f.executor.clone();
    let handle = tokio::spawn(async move { ex.run("t24").await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.agent.total_calls() >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("第一次 run 应进行到 LLM 调用");
    // 第二次 run：进程内注册表命中 → 立即返回，无额外 LLM 调用
    f.executor.run("t24").await.unwrap();
    assert_eq!(f.agent.total_calls(), 1, "单执行者守卫阻止重复执行");
    handle.abort();
}
