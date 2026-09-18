//! E2E 依赖 / 准入 / 取消传播场景（testing.md §8）：E2E-19 依赖三态、E2E-20 并发准入、
//! E2E-23 取消传播（core 可观测部分）。
//!
//! 决策 57 / 98 / 116 / 117 / §12.3。

use super::common::Flow;
use agentpipeline_core::config::Settings;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    Node, PendingKind, ReviewMode, Stage, TaskStatus, TransitionTrigger,
};

// ─────────────────────────── E2E-19 ───────────────────────────

#[tokio::test]
async fn e2e_19_dependency_three_states_waiting_queued_running() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "dep-ok", "p1").await.unwrap();
    let w = testkit::seed_task_full(&f.store, "w19", "p1", ReviewMode::Agent, &["dep-ok"])
        .await
        .unwrap();
    assert_eq!(w.status, TaskStatus::Waiting, "有依赖 → waiting");

    // 依赖未完成：不提升
    let report = f.scheduler().tick().await.unwrap();
    assert!(report.dependencies_promoted.is_empty());

    // 依赖 done → queued，同一 tick 内被准入 → running
    f.store
        .set_task_status("dep-ok", TaskStatus::Done)
        .await
        .unwrap();
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.dependencies_promoted, vec!["w19".to_string()]);
    assert_eq!(
        f.store.get_task("w19").await.unwrap().status,
        TaskStatus::Running,
        "queued 后同 tick 准入 → running"
    );
}

#[tokio::test]
async fn e2e_19_dep_failed_pends_main_cursor_continue_and_retry_recover() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "dep", "p1").await.unwrap();
    testkit::seed_task_full(&f.store, "w19b", "p1", ReviewMode::Agent, &["dep"])
        .await
        .unwrap();
    f.store
        .set_task_status("dep", TaskStatus::Failed)
        .await
        .unwrap();
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.dependency_failed, vec!["w19b".to_string()]);

    // pending(dependency_failed) 挂 main 游标；动作集含「等待依赖重试」
    let cursor = f.store.resolve_sole_cursor("w19b").await.unwrap().unwrap();
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::DependencyFailed);
    assert_eq!((reason.stage, reason.node), (Stage::Init, Node::Execute));
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("dependency_failed")
    );
    let names: Vec<String> = agentpipeline_core::actions::allowed_actions(reason, None)
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert!(names.contains(&"wait_dependency_retry".to_string()));
    assert!(names.contains(&"continue".to_string()));

    // continue = 忽略失败依赖 → 置回 queued 重新准入（决策 116）
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    assert_eq!(
        f.store.get_task("w19b").await.unwrap().status,
        TaskStatus::Queued,
        "continue → queued（决策 116）"
    );

    // 决策 116 / 票 06：continue 必须在观测面留下 `dependency_overridden` 警告，
    // 含被忽略的依赖任务 id——否则事后看不出这个任务是踩着失败依赖上路的。
    let transitions = f.store.list_transitions("w19b").await.unwrap();
    assert!(
        transitions.iter().any(|t| {
            t.trigger == TransitionTrigger::UserResume
                && t.reason
                    .as_deref()
                    .unwrap_or("")
                    .contains("dependency_overridden")
                && t.reason.as_deref().unwrap_or("").contains("dep")
        }),
        "continue 应落 dependency_overridden 警告且含依赖 id：{transitions:?}"
    );

    // 依赖重试转 running → 从 pending 退回 waiting（决策 57）：用新的依赖方建模
    testkit::seed_task(&f.store, "dep2", "p1").await.unwrap();
    testkit::seed_task_full(&f.store, "w19d", "p1", ReviewMode::Agent, &["dep2"])
        .await
        .unwrap();
    f.store
        .set_task_status("dep2", TaskStatus::Failed)
        .await
        .unwrap();
    f.scheduler().tick().await.unwrap();
    assert_eq!(
        f.store.get_task("w19d").await.unwrap().status,
        TaskStatus::Pending
    );
    f.store
        .set_task_status("dep2", TaskStatus::Running)
        .await
        .unwrap();
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.dependencies_recovered, vec!["w19d".to_string()]);
    assert_eq!(
        f.store.get_task("w19d").await.unwrap().status,
        TaskStatus::Waiting
    );
}

#[tokio::test]
async fn e2e_19_dep_cancelled_drops_wait_for_retry_action() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "dep-x", "p1").await.unwrap();
    testkit::seed_task_full(&f.store, "w19c", "p1", ReviewMode::Agent, &["dep-x"])
        .await
        .unwrap();
    f.store
        .set_task_status("dep-x", TaskStatus::Cancelled)
        .await
        .unwrap();
    f.scheduler().tick().await.unwrap();

    let cursor = f.store.resolve_sole_cursor("w19c").await.unwrap().unwrap();
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("dependency_cancelled")
    );
    let names: Vec<String> = agentpipeline_core::actions::allowed_actions(reason, None)
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert!(
        !names.contains(&"wait_dependency_retry".to_string()),
        "依赖已取消 → 无「等待依赖重试」（决策 116）"
    );
    assert_eq!(names, vec!["continue", "cancel"]);

    // 决策 116 / 票 06：cancelled 分支的 continue 同样落 dependency_overridden 警告
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    let transitions = f.store.list_transitions("w19c").await.unwrap();
    assert!(
        transitions.iter().any(|t| {
            t.trigger == TransitionTrigger::UserResume
                && t.reason
                    .as_deref()
                    .unwrap_or("")
                    .contains("dependency_overridden")
                && t.reason.as_deref().unwrap_or("").contains("dep-x")
        }),
        "cancelled 分支 continue 也应落警告且含依赖 id：{transitions:?}"
    );
}

// ─────────────────────────── E2E-20 ───────────────────────────

#[tokio::test]
async fn e2e_20_concurrent_admission_respects_max_and_slot_release() {
    let settings = Settings {
        max_concurrent_tasks: 1,
        ..Default::default()
    };
    let f = Flow::with_settings(settings.clone()).await;
    f.store
        .create_task(&agentpipeline_core::storage::tasks::NewTask::new(
            "t1",
            "任务 t1",
            "p1",
        ))
        .await
        .unwrap();
    f.clock.advance_secs(1);
    f.store
        .create_task(&agentpipeline_core::storage::tasks::NewTask::new(
            "t2",
            "任务 t2",
            "p1",
        ))
        .await
        .unwrap();

    // max=1：只放行一个，另一个停 queued
    let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t1".to_string()]);
    assert_eq!(f.store.occupying_slots().await.unwrap(), 1);
    assert_eq!(
        f.store.get_task("t2").await.unwrap().status,
        TaskStatus::Queued
    );

    // pending 仍占名额 → 不再放行（决策 117）
    f.store
        .set_task_status("t1", TaskStatus::Pending)
        .await
        .unwrap();
    let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
    assert!(report.admitted.is_empty(), "pending 占名额");

    // 终态释放名额 → 放行下一个
    f.store
        .set_task_status("t1", TaskStatus::Done)
        .await
        .unwrap();
    let report = f.scheduler_with(settings.clone()).tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t2".to_string()]);
    assert_eq!(
        f.store.get_task("t2").await.unwrap().status,
        TaskStatus::Running
    );

    // failed → retry 须置回 queued 重新准入（决策 117）
    f.store
        .set_task_status("t2", TaskStatus::Failed)
        .await
        .unwrap();
    f.store
        .set_task_status("t2", TaskStatus::Queued)
        .await
        .unwrap();
    let report = f.scheduler_with(settings).tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t2".to_string()], "retry 重新准入");
}

// ─────────────────────────── E2E-23（core 可观测部分）───────────────────────────

#[tokio::test]
async fn e2e_23_cancel_propagates_dependency_failed_to_unstarted_dependents() {
    // 取消清理 worktree / 分支 + SSE task_cancelled 由 app 层接线（crates/app/src/routes/tasks.rs，
    // L3 API 契约覆盖，见 ticket 19 报告）；此处锁定 core 的依赖传播语义（§12.3）。
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "cancel-me", "p1")
        .await
        .unwrap();
    testkit::seed_task_full(&f.store, "blocked", "p1", ReviewMode::Agent, &["cancel-me"])
        .await
        .unwrap();

    let notified = f.store.cancel_task("cancel-me").await.unwrap();
    assert_eq!(notified, vec!["blocked".to_string()]);
    assert_eq!(
        f.store.get_task("cancel-me").await.unwrap().status,
        TaskStatus::Cancelled
    );
    let cursor = f
        .store
        .resolve_sole_cursor("blocked")
        .await
        .unwrap()
        .unwrap();
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::DependencyFailed);
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("dependency_cancelled")
    );
    assert_eq!(
        f.store.get_task("blocked").await.unwrap().status,
        TaskStatus::Pending
    );
}
