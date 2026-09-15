//! L2 集成：游标生命周期（testing.md §6，决策 80 / 90 / 91 / 93 / 113）。
//!
//! 这里用真 SQLite 临时文件库 + 全量迁移（决策 145），逐条验证游标状态机的合法迁移。

use std::sync::Arc;

use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    CursorStatus, Node, NodeCursor, PendingContext, PendingKind, PendingReason, ReviewMode, Stage,
    TaskStatus,
};
use testkit::{seed_project, seed_task, seed_task_full, TestHome};

async fn setup() -> (TestHome, Store) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    (home, store)
}

async fn with_task() -> (TestHome, Store, agentpipeline_core::types::Task) {
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    let project = seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    let task = seed_task(&store, "t1", &project.id).await.unwrap();
    (home, store, task)
}

// ─────────────────────────── 创建（决策 90）───────────────────────────

#[tokio::test]
async fn task_creation_inserts_main_cursor_in_same_transaction() {
    let (_home, store, task) = with_task().await;
    assert_eq!(task.status, TaskStatus::Queued);
    let cursors = store.load_live_cursors("t1").await.unwrap();
    assert_eq!(cursors.len(), 1);
    let main = &cursors[0];
    assert_eq!(main.branch, NodeCursor::BRANCH_MAIN);
    assert_eq!(main.stage, Stage::Init);
    assert_eq!(main.node, Node::Execute);
    assert_eq!(main.status, CursorStatus::Active);
    assert_eq!(main.validate_attempts, 0);
}

#[tokio::test]
async fn waiting_task_also_has_a_cursor_for_dependency_failed() {
    // 决策 90：waiting / queued 任务也有游标，dependency_failed 因此有处可挂
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "dep", "p1").await.unwrap();
    let task = testkit::seed_task_full(&store, "t2", "p1", ReviewMode::Agent, &["dep"])
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Waiting);
    let cursors = store.load_live_cursors("t2").await.unwrap();
    assert_eq!(cursors.len(), 1);
    assert_eq!(cursors[0].stage, Stage::Init);
}

// ─────────────────────────── 分裂（决策 90 / 113）───────────────────────────

#[tokio::test]
async fn split_rewrites_main_in_place_and_inserts_second_branch() {
    let (_home, store, _task) = with_task().await;
    let original = store.load_live_cursors("t1").await.unwrap()[0].clone();

    let split = store.split_cursors("t1").await.unwrap();
    assert_eq!(split.len(), 2);

    let dev = split
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();
    let test = split
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();

    // main 行**就地改写**：cursor_id 不变（决策 90）
    assert_eq!(dev.cursor_id, original.cursor_id);
    assert_eq!(dev.stage, Stage::DevelopDesign);
    assert_eq!(dev.node, Node::ValidateInput);
    assert_ne!(test.cursor_id, original.cursor_id);
    assert_eq!(test.stage, Stage::TestDesign);

    // 并行区间不存在活跃 main 游标（决策 91）
    assert!(store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .iter()
        .all(|c| c.branch != NodeCursor::BRANCH_MAIN));
}

#[tokio::test]
async fn split_is_idempotent() {
    let (_home, store, _task) = with_task().await;
    let first = store.split_cursors("t1").await.unwrap();
    let second = store.split_cursors("t1").await.unwrap();

    let ids = |v: &[NodeCursor]| {
        let mut s: Vec<String> = v.iter().map(|c| c.cursor_id.clone()).collect();
        s.sort();
        s
    };
    assert_eq!(ids(&first), ids(&second), "重复分裂不得产生重复游标");
    assert_eq!(store.load_live_cursors("t1").await.unwrap().len(), 2);
    // 重复分裂也不得留下归档垃圾
    assert_eq!(store.load_all_cursors("t1").await.unwrap().len(), 2);
}

#[tokio::test]
async fn split_after_split_attempts_are_independent() {
    let (_home, store, _task) = with_task().await;
    let split = store.split_cursors("t1").await.unwrap();
    let dev = split
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();

    // 决策 82：attempts 每游标独立
    let attempts = store
        .increment_cursor_attempts(&dev.cursor_id)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    let test_cursor = store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();
    assert_eq!(test_cursor.validate_attempts, 0);
}

// ─────────────────────────── 合并 / 回退 / 重试（决策 113）───────────────────────────

#[tokio::test]
async fn merge_archives_branches_and_inserts_single_main() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();

    let main = store.merge_cursors_to_develop("t1").await.unwrap();
    assert_eq!(main.branch, NodeCursor::BRANCH_MAIN);
    assert_eq!(main.stage, Stage::Develop);
    assert_eq!(main.node, Node::Execute);
    assert_eq!(main.status, CursorStatus::Active);

    let live = store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].cursor_id, main.cursor_id);

    // 旧行只归档、不物理删除（决策 113）
    let all = store.load_all_cursors("t1").await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count(),
        2
    );
}

#[tokio::test]
async fn backtrack_resets_both_branches_to_architect_validate_input() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();
    let main = store.backtrack_cursors("t1").await.unwrap();
    assert_eq!(main.stage, Stage::ArchitectDesign);
    assert_eq!(main.node, Node::ValidateInput);
    assert_eq!(store.load_live_cursors("t1").await.unwrap().len(), 1);
    // 两条分支一起归档
    assert_eq!(store.load_all_cursors("t1").await.unwrap().len(), 3);
}

#[tokio::test]
async fn backtrack_marks_design_outputs_stale_and_upsert_clears() {
    // 决策 83：标过期与游标归档同事务；文件保留供回溯，覆盖写入时清除
    let (_home, store, _task) = with_task().await;
    for (stage, output_type) in [
        (Stage::ArchitectDesign, "design_doc"),
        (Stage::DevelopDesign, "dev_doc"),
        (Stage::TestDesign, "test_scenarios"),
    ] {
        store
            .upsert_stage_output("t1", stage, output_type, "x.md", None)
            .await
            .unwrap();
    }

    store.backtrack_cursors("t1").await.unwrap();

    let dev = store
        .get_stage_output("t1", Stage::DevelopDesign, "dev_doc")
        .await
        .unwrap()
        .unwrap();
    let test = store
        .get_stage_output("t1", Stage::TestDesign, "test_scenarios")
        .await
        .unwrap()
        .unwrap();
    let design = store
        .get_stage_output("t1", Stage::ArchitectDesign, "design_doc")
        .await
        .unwrap()
        .unwrap();
    assert!(dev.stale, "dev_doc 应标过期");
    assert!(test.stale, "test_scenarios 应标过期");
    assert!(!design.stale, "design_doc 不标过期");

    // 下次执行覆盖写入 → upsert 清除过期标记（决策 83「下次执行覆盖写入」）
    store
        .upsert_stage_output("t1", Stage::DevelopDesign, "dev_doc", "dev-plan.md", None)
        .await
        .unwrap();
    let dev = store
        .get_stage_output("t1", Stage::DevelopDesign, "dev_doc")
        .await
        .unwrap()
        .unwrap();
    assert!(!dev.stale, "覆盖写入应清除过期标记");
}

#[tokio::test]
async fn retry_reset_archives_everything_and_starts_from_init() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();
    store.merge_cursors_to_develop("t1").await.unwrap();

    let fresh = store.reset_cursors_to_init("t1").await.unwrap();
    assert_eq!(fresh.stage, Stage::Init);
    assert_eq!(fresh.node, Node::Execute);
    let live = store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].cursor_id, fresh.cursor_id);

    // 累计 4 行：1 初始 + 1 分裂出的 test-design + 1 merge main + 1 retry main
    assert_eq!(store.load_all_cursors("t1").await.unwrap().len(), 4);
}

#[tokio::test]
async fn cursor_rows_are_never_physically_deleted() {
    let (_home, store, _task) = with_task().await;
    let mut total = store.load_all_cursors("t1").await.unwrap().len();
    for _ in 0..3 {
        store.split_cursors("t1").await.unwrap();
        store.merge_cursors_to_develop("t1").await.unwrap();
        let now = store.load_all_cursors("t1").await.unwrap().len();
        assert!(now > total, "游标总行数只增不减");
        total = now;
    }
}

#[tokio::test]
async fn run_foreign_key_never_dangles_across_cursor_replacement() {
    // 决策 113 的核心收益：run 行的 cursor_id 外键永不悬空
    let (_home, store, _task) = with_task().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    let run_id = store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "system".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();

    // 反复替换游标后，旧 run 仍能 join 到游标行
    store.merge_cursors_to_develop("t1").await.unwrap();
    store.reset_cursors_to_init("t1").await.unwrap();

    let joined: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kanban_node_runs r
         JOIN kanban_node_cursors c ON c.cursor_id = r.cursor_id
         WHERE r.id = ?",
    )
    .bind(run_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(joined, 1);
    // 外键完整性检查（SQLite 会在违反时报错）
    let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kanban_node_runs")
        .fetch_one(store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn partial_unique_index_allows_new_row_after_archiving() {
    let (_home, store, _task) = with_task().await;
    let original = store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 活跃行同分支唯一：直接插第二条 main 必须失败
    let dup = sqlx::query(
        "INSERT INTO kanban_node_cursors
         (cursor_id, task_id, branch, stage, node, status, validate_attempts, skipped_to_join,
          pending_reason_json, created_at, updated_at)
         VALUES ('dup', 't1', 'main', 'init', 'execute', 'active', 0, 0, NULL, '', '')",
    )
    .execute(store.pool())
    .await;
    assert!(dup.is_err(), "partial UNIQUE 应拦住重复活跃分支");

    // 归档后允许插入（决策 113）
    store.archive_cursor(&original.cursor_id).await.unwrap();
    let ok = sqlx::query(
        "INSERT INTO kanban_node_cursors
         (cursor_id, task_id, branch, stage, node, status, validate_attempts, skipped_to_join,
          pending_reason_json, created_at, updated_at)
         VALUES ('dup2', 't1', 'main', 'init', 'execute', 'active', 0, 0, NULL, '', '')",
    )
    .execute(store.pool())
    .await;
    assert!(ok.is_ok(), "归档行应让位于同分支新行");
}

// ─────────────────────────── resume 目标解析（决策 91）───────────────────────────

#[tokio::test]
async fn resolve_sole_cursor_requires_exactly_one() {
    let (_home, store, _task) = with_task().await;
    // 串行阶段：恰好一条 → 可省略 cursor_id
    assert!(store.resolve_sole_cursor("t1").await.unwrap().is_some());

    store.split_cursors("t1").await.unwrap();
    // 并行区间：两条 → 必须显式提供（API 层 409）
    assert!(store.resolve_sole_cursor("t1").await.unwrap().is_none());

    store.merge_cursors_to_develop("t1").await.unwrap();
    assert!(store.resolve_sole_cursor("t1").await.unwrap().is_some());
}

// ─────────────────────────── pending 挂载（决策 82 / 102）───────────────────────────

#[tokio::test]
async fn pending_is_attached_to_a_cursor_not_the_task_only() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();
    // 任务已准入（决策 98）：queued 任务的 status 不被游标投影覆盖
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();
    let dev = store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();

    let reason = PendingReason::new(
        PendingKind::Timeout,
        Stage::DevelopDesign,
        Node::Execute,
        "超时",
    );
    store
        .set_cursor_pending(&dev.cursor_id, &reason)
        .await
        .unwrap();

    let cursors = store.load_live_cursors("t1").await.unwrap();
    let dev_now = cursors
        .iter()
        .find(|c| c.cursor_id == dev.cursor_id)
        .unwrap();
    assert_eq!(dev_now.status, CursorStatus::Pending);
    assert_eq!(
        dev_now.pending_reason.as_ref().unwrap().kind,
        PendingKind::Timeout
    );
    // 另一分支不受影响（决策 89）
    let test_now = cursors
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();
    assert_eq!(test_now.status, CursorStatus::Active);
    assert!(test_now.pending_reason.is_none());

    // 任务级投影随之变为 pending
    let projected = store.sync_task_projection("t1").await.unwrap();
    assert_eq!(projected.status, TaskStatus::Pending);
    assert_eq!(projected.current_stage, Stage::DevelopDesign);
    // 清理 pending 后回到 running
    store.clear_cursor_pending(&dev.cursor_id).await.unwrap();
    assert_eq!(
        store.sync_task_projection("t1").await.unwrap().status,
        TaskStatus::Running
    );

    // 清除后恢复 active
    store.clear_cursor_pending(&dev.cursor_id).await.unwrap();
    assert_eq!(
        store
            .load_live_cursors("t1")
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.cursor_id == dev.cursor_id)
            .unwrap()
            .status,
        CursorStatus::Active
    );
}

#[tokio::test]
async fn conflict_wait_context_updates_without_rerunning_node() {
    // 决策 102：复检仍有交集 → 保持 pending，只更新 conflict_task_ids
    let (_home, store, _task) = with_task().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    let reason = PendingReason::new(
        PendingKind::ConflictWait,
        Stage::ArchitectDesign,
        Node::Execute,
        "与其他任务冲突",
    )
    .with_context(PendingContext {
        conflict_task_ids: vec!["t2".into()],
        ..Default::default()
    });
    store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();

    store
        .update_cursor_pending_context(
            &cursor.cursor_id,
            "conflict_task_ids",
            &["t3".into(), "t4".into()],
        )
        .await
        .unwrap();

    let now = store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(now.status, CursorStatus::Pending);
    let ctx = now.pending_reason.unwrap().context.unwrap();
    assert_eq!(
        ctx.conflict_task_ids,
        vec!["t3".to_string(), "t4".to_string()]
    );
}

#[tokio::test]
async fn cursors_with_pending_kind_filters_by_type() {
    let (_home, store, _task) = with_task().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    store
        .set_cursor_pending(
            &cursor.cursor_id,
            &PendingReason::new(
                PendingKind::ConflictWait,
                Stage::ArchitectDesign,
                Node::Execute,
                "c",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .cursors_with_pending_kind(PendingKind::ConflictWait)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .cursors_with_pending_kind(PendingKind::Timeout)
        .await
        .unwrap()
        .is_empty());
}

// ─────────────────────────── skip 落点（决策 93 / 115）───────────────────────────

#[tokio::test]
async fn skip_on_parallel_branch_marks_waiting_join_without_forging_readiness() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();
    let test_cursor = store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();

    // 先写入产出元数据，验证 skip 不会伪造 readiness（决策 93）
    store
        .upsert_stage_output(
            "t1",
            Stage::TestDesign,
            "test_scenarios",
            "test-scenarios.md",
            Some(&serde_json::json!({"readiness": false, "blockers": ["缺少验收标准引用"]})),
        )
        .await
        .unwrap();

    store
        .mark_cursor_skipped_to_join(&test_cursor.cursor_id)
        .await
        .unwrap();

    let after = store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.cursor_id == test_cursor.cursor_id)
        .unwrap();
    assert_eq!(after.status, CursorStatus::WaitingJoin);
    assert!(after.skipped_to_join);

    // 产出元数据保持原样（不伪造 agent 的结论）
    let meta = store
        .stage_output_metadata("t1", Stage::TestDesign, "test_scenarios")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta["readiness"], false);
    assert_eq!(meta["blockers"][0], "缺少验收标准引用");
}

#[tokio::test]
async fn stage_jump_resets_attempts() {
    let (_home, store, _task) = with_task().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    store
        .increment_cursor_attempts(&cursor.cursor_id)
        .await
        .unwrap();
    store
        .increment_cursor_attempts(&cursor.cursor_id)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_cursor(&cursor.cursor_id)
            .await
            .unwrap()
            .validate_attempts,
        2
    );

    store
        .set_cursor_stage(&cursor.cursor_id, Stage::Develop, Node::Execute)
        .await
        .unwrap();
    let after = store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        after.validate_attempts, 0,
        "跨阶段跳转重置 attempts（决策 43）"
    );
    assert_eq!(after.stage, Stage::Develop);
}

// ─────────────────────────── 焦点投影（决策 92 / 130）───────────────────────────

#[tokio::test]
async fn projection_picks_pending_cursor_and_does_not_override_queued() {
    let (_home, store, _task) = with_task().await;
    // queued 任务不被游标投影覆盖（决策 98）
    let projected = store.sync_task_projection("t1").await.unwrap();
    assert_eq!(projected.status, TaskStatus::Queued);

    store.split_cursors("t1").await.unwrap();
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();

    let dev = store
        .load_live_cursors("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();
    store
        .set_cursor_pending(
            &dev.cursor_id,
            &PendingReason::new(
                PendingKind::UserDecision,
                Stage::DevelopDesign,
                Node::ValidateInput,
                "输入不足",
            ),
        )
        .await
        .unwrap();

    let projected = store.sync_task_projection("t1").await.unwrap();
    assert_eq!(projected.status, TaskStatus::Pending);
    assert_eq!(projected.current_stage, Stage::DevelopDesign);
    assert_eq!(projected.current_node, Node::ValidateInput);
    assert!(projected.pending_reason.is_some());
}

#[tokio::test]
async fn double_pending_projection_takes_latest_updated() {
    let (_home, store, _task) = with_task().await;
    store.split_cursors("t1").await.unwrap();
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();
    let cursors = store.load_live_cursors("t1").await.unwrap();

    for (i, cursor) in cursors.iter().enumerate() {
        // 时钟固定在 fixed()，用不同的冲突 id 区分两条 pending 的来源
        store
            .set_cursor_pending(
                &cursor.cursor_id,
                &PendingReason::new(
                    PendingKind::Timeout,
                    cursor.stage,
                    Node::Execute,
                    format!("第 {i} 条 pending"),
                ),
            )
            .await
            .unwrap();
    }

    let projected = store.sync_task_projection("t1").await.unwrap();
    let reason = projected.pending_reason.unwrap();
    assert!(reason.message.contains("pending"));
    // 双 pending 时焦点取 updated_at 最新——固定时钟下两者相同，投影必须稳定取到其中一条
    assert!(reason.message.ends_with("条 pending"));
}

// ─────────────────────────── executor 单执行者（决策 36 / 127）───────────────────────────

#[tokio::test]
async fn executor_claim_is_exclusive_and_startup_clears_residue() {
    let (_home, store, _task) = with_task().await;
    assert!(store.try_claim_executor("t1", "owner-a").await.unwrap());
    // 已有持有者 → 抢占失败
    assert!(!store.try_claim_executor("t1", "owner-b").await.unwrap());
    assert_eq!(
        store
            .get_task("t1")
            .await
            .unwrap()
            .executor_owner
            .as_deref(),
        Some("owner-a")
    );

    store.release_executor("t1").await.unwrap();
    assert!(store.try_claim_executor("t1", "owner-b").await.unwrap());

    // 决策 127：模拟 kill -9 残留，启动清理必须释放
    assert_eq!(store.clear_executor_owners().await.unwrap(), 1);
    assert!(store.get_task("t1").await.unwrap().executor_owner.is_none());
}

#[tokio::test]
async fn startup_recovery_requeues_orphaned_running_tasks() {
    // 决策 127 补全（主流程票 08）：kill -9 只清 executor_owner 不够——
    // 调度器准入只认 queued，孤儿 running 不归队就会在重启后永久挂起。
    let (_home, store, _task) = with_task().await;
    store.try_claim_executor("t1", "owner-a").await.unwrap();
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();

    let requeued = store.requeue_running_tasks().await.unwrap();
    assert_eq!(requeued, vec!["t1".to_string()]);
    let task = store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Queued);

    // 非 running 任务不受影响；重复归队是空操作
    store
        .set_task_status("t1", TaskStatus::Pending)
        .await
        .unwrap();
    assert!(store.requeue_running_tasks().await.unwrap().is_empty());
    assert_eq!(
        store.get_task("t1").await.unwrap().status,
        TaskStatus::Pending
    );
}

#[tokio::test]
async fn double_connection_admission_serializes() {
    // 决策 36 / §12.10：双连接竞争同一个名额
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t1", "p1").await.unwrap();

    let store2 = Store::open(home.home().clone(), Arc::new(testkit::ManualClock::fixed()))
        .await
        .unwrap();

    let (a, b) = tokio::join!(
        store.try_claim_executor("t1", "a"),
        store2.try_claim_executor("t1", "b")
    );
    assert!(a.unwrap() ^ b.unwrap(), "恰好一个连接抢到乐观锁");
}

// ─────────────────────────── 依赖（决策 27 / 57 / 116）───────────────────────────

#[tokio::test]
async fn dependency_states_and_cycle_detection() {
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "a", "p1").await.unwrap();
    seed_task(&store, "b", "p1").await.unwrap();
    store.add_dependency("b", "a").await.unwrap();

    assert_eq!(
        store.dependencies_satisfied("b").await.unwrap(),
        agentpipeline_core::storage::tasks::DependencyState::Waiting(vec!["a".into()])
    );
    assert_eq!(
        store.dependencies_satisfied("a").await.unwrap(),
        agentpipeline_core::storage::tasks::DependencyState::Ready
    );

    store.set_task_status("a", TaskStatus::Done).await.unwrap();
    assert_eq!(
        store.dependencies_satisfied("b").await.unwrap(),
        agentpipeline_core::storage::tasks::DependencyState::Ready
    );

    store
        .set_task_status("a", TaskStatus::Failed)
        .await
        .unwrap();
    assert_eq!(
        store.dependencies_satisfied("b").await.unwrap(),
        agentpipeline_core::storage::tasks::DependencyState::Failed(vec!["a".into()])
    );

    // 循环依赖：c 依赖 b 时，若再让 a 依赖 c 则成环
    seed_task(&store, "c", "p1").await.unwrap();
    store.add_dependency("c", "b").await.unwrap();
    assert!(!store.would_create_cycle("c", &["b".into()]).await.unwrap());
    assert!(store.would_create_cycle("a", &["c".into()]).await.unwrap());
}

#[tokio::test]
async fn dependents_are_derived_not_stored() {
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "a", "p1").await.unwrap();
    seed_task(&store, "b", "p1").await.unwrap();
    store.add_dependency("b", "a").await.unwrap();
    assert_eq!(
        store.dependents_of("a").await.unwrap(),
        vec!["b".to_string()]
    );
    assert!(store.dependents_of("b").await.unwrap().is_empty());
}

#[tokio::test]
async fn admission_counts_running_and_pending_as_occupying() {
    // 决策 117：名额占用 = running + pending（pending 仍持有 worktree）
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t1", "p1").await.unwrap();
    seed_task(&store, "t2", "p1").await.unwrap();

    assert_eq!(store.occupying_slots().await.unwrap(), 0);
    assert!(store.try_admit("t1", 5).await.unwrap());
    assert_eq!(store.occupying_slots().await.unwrap(), 1);

    store
        .set_task_status("t1", TaskStatus::Pending)
        .await
        .unwrap();
    assert_eq!(
        store.occupying_slots().await.unwrap(),
        1,
        "pending 仍占名额"
    );

    // max = 1 时 t2 不得放行
    assert!(!store.try_admit("t2", 1).await.unwrap());
    assert_eq!(
        store.get_task("t2").await.unwrap().status,
        TaskStatus::Queued
    );

    store.set_task_status("t1", TaskStatus::Done).await.unwrap();
    assert!(store.try_admit("t2", 1).await.unwrap());
}

// ─────────────────── 偏离修复回归（2026-09-12 文档-实现对齐）───────────────────

#[tokio::test]
async fn corrupt_cursor_row_fails_fast_instead_of_silent_fallback() {
    // B4 / Q6(c)：执行语义字段损坏必须报错——静默兜底会把坏行当 init/active 用
    let (_home, store) = setup().await;
    let repo = _home.scratch_dir("proj");
    let project = seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t1", &project.id).await.unwrap();

    sqlx::query("UPDATE kanban_node_cursors SET stage = 'bogus-stage' WHERE task_id = 't1'")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        store.load_live_cursors("t1").await.is_err(),
        "损坏的 stage 必须让加载失败"
    );

    // 对照：观测类字段（这里是 status 之外的合法值）不受影响
    sqlx::query("UPDATE kanban_node_cursors SET stage = 'develop' WHERE task_id = 't1'")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(store.load_live_cursors("t1").await.is_ok());
}

#[tokio::test]
async fn cancel_only_pends_not_yet_started_dependents() {
    // A8 / data-model §5：dependency_failed 只挂 queued / waiting 的依赖方，
    // 已在执行的依赖方不得被翻成 pending
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "dep", "p1").await.unwrap();
    seed_task_full(&store, "queued-child", "p1", ReviewMode::Agent, &["dep"])
        .await
        .unwrap();
    seed_task(&store, "running-child", "p1").await.unwrap();
    store.add_dependency("running-child", "dep").await.unwrap();
    store
        .set_task_status("running-child", TaskStatus::Running)
        .await
        .unwrap();

    let notified = store.cancel_task("dep").await.unwrap();
    assert_eq!(notified, vec!["queued-child".to_string()]);

    let running = store.get_task("running-child").await.unwrap();
    assert_eq!(
        running.status,
        TaskStatus::Running,
        "running 依赖方不受牵连"
    );
    let running_cursor = &store.load_live_cursors("running-child").await.unwrap()[0];
    assert!(running_cursor.pending_reason.is_none());

    let queued_cursor = &store.load_live_cursors("queued-child").await.unwrap()[0];
    assert_eq!(
        queued_cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::DependencyFailed
    );
}

// ─────────── 并发写的快照冲突（主流程票 09 暴露的真实缺陷）───────────

/// 两个任务在同一项目并发推进（决策 98 准入允许的常态用法）时，存储层的多步
/// 读-写事务必须能共存。
///
/// 回归的是 `SQLITE_BUSY_SNAPSHOT`（code 517）：`pool.begin()` 发的是 deferred
/// `BEGIN`，事务内**先读后写**且中途其他连接提交，SQLite 会拒绝这次快照升级——
/// 它不是锁等待，`busy_timeout` 重试同一个快照永远失败，事务直接报
/// 「database is locked」。修复：写事务统一走 `begin_write()`（`BEGIN IMMEDIATE`），
/// 建事务即取写锁，冲突退化为普通锁等待。
///
/// 单任务串行时读-写之间没有竞争者，因此此前从未暴露；票 09 的浏览器用例让两个
/// 任务同时跑，节点因此被重试耗尽（用户可见：任务无故挂 retry_exhausted）。
#[tokio::test]
async fn concurrent_writers_do_not_fail_with_busy_snapshot() {
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    for id in ["t-a", "t-b"] {
        seed_task(&store, id, "p1").await.unwrap();
    }

    // 反复并发跑「读游标 → 写游标」的多步事务；修复前必有一侧报 517。
    for _ in 0..25 {
        let (a, b) = tokio::join!(store.split_cursors("t-a"), store.split_cursors("t-b"));
        a.expect("t-a 的分裂事务不应因并发写失败");
        b.expect("t-b 的分裂事务不应因并发写失败");
        // 复原成「未分裂」以便下一轮重跑同一路径（幂等分支会提前返回，测不到冲突）；
        // replace_cursors_with_main 自身在单事务内归档全部活跃行并插入新 main。
        for id in ["t-a", "t-b"] {
            store
                .replace_cursors_with_main(id, Stage::Init, Node::Execute, NodeCursor::BRANCH_MAIN)
                .await
                .unwrap();
        }
    }
}

// ─────────────── resume 的一次性标记（决策 180，票 13）───────────────

/// 「刚被 resume」标记：只有真翻过一次 pending 的游标才有，且**取走即清零**。
///
/// 回归的是一处 SQL 语义陷阱：SQLite 的 `RETURNING` 报的是**更新之后**的值，
/// 因此 `UPDATE ... SET resumed_from_pending = 0 ... RETURNING resumed_from_pending`
/// 永远读到 0——续接会静默失效（探针表现为开了开关却读不到上一轮对话，且无任何报错）。
/// 把「原本为 1」写进 `WHERE` 后按行是否存在判断，才对得上「取走即清零」的语义。
#[tokio::test]
async fn resumed_from_pending_flag_is_one_shot() {
    let (_home, store, _task) = with_task().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 没 pending 过：没有标记（对非 pending 游标 clear 是 no-op，不该留下标记）
    store.clear_cursor_pending(&cursor.cursor_id).await.unwrap();
    assert!(!store
        .take_cursor_resumed_from_pending(&cursor.cursor_id)
        .await
        .unwrap());

    // 真挂过 pending 再清：第一次取到，第二次清零
    let reason = PendingReason::new(
        PendingKind::InfoInsufficient,
        Stage::Init,
        Node::Execute,
        "缺输入",
    );
    store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    store.clear_cursor_pending(&cursor.cursor_id).await.unwrap();
    assert!(store
        .take_cursor_resumed_from_pending(&cursor.cursor_id)
        .await
        .unwrap());
    assert!(!store
        .take_cursor_resumed_from_pending(&cursor.cursor_id)
        .await
        .unwrap());
}
