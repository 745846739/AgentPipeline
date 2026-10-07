//! L2 集成（决策 328）：删除项目 = **连它的全部历史一起删干净**。
//!
//! 症状（2026-09-29 实测）：`DELETE /projects/{id}` 报
//! `FOREIGN KEY constraint failed`（SQLite code 787）。根因是 `delete_project`
//! 只删父行，而三张 `NO ACTION` 外键子表（`kanban_project_analyses` /
//! `kanban_node_runs` / `kanban_node_conversations`）挂着它；反过来
//! `kanban_tasks.project_id` **没有**外键，父行一删任务就成孤儿继续留在看板上。
//!
//! 这里钉三件事：① 不留行（逐表扫，含孤儿任务）；② 不越界（别的项目一行不动，
//! 跨项目的依赖边被清掉）；③ 决策 101 的活跃任务闸门仍是联锁，且拒绝时**什么都没删**。

use agentpipeline_core::storage::attention::AttentionKind;
use agentpipeline_core::storage::observability::{NewProjectRun, NewRun};
use agentpipeline_core::storage::tasks::NewTask;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Node, Stage, TaskStatus, TransitionTrigger};
use testkit::{seed_project, TestHome};

const PSEUDO_PROJECT_ANALYSIS: &str = "pseudo:project_analysis";

async fn setup() -> (TestHome, Store) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    (home, store)
}

/// 把任务造成「跑过一轮」的样子：游标 + 父 run + 子 run + 命令 + 会话 + 流转 +
/// 阶段产出 + 待办 + 模型请求。返回任务 id。
///
/// 覆盖的是**表**而不是某条业务路径——这个 bug 的形状就是「有哪张表忘了清」，
/// 所以造的每一行都对应删除路径里必须处理的一张子表。
async fn seed_used_task(store: &Store, task_id: &str, project_id: &str) -> i64 {
    store
        .create_task(&NewTask::new(
            task_id,
            format!("任务 {task_id}"),
            project_id,
        ))
        .await
        .unwrap();
    let cursor = store
        .load_live_cursors(task_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("create_task 应落一条初始游标");

    let parent = store
        .insert_run(&NewRun {
            task_id: task_id.into(),
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
    // 子 run（`parent_run_id` 自引用）：删除时必须先断链，否则同一批里父行先走就报错。
    let child = store
        .insert_run(&NewRun {
            task_id: task_id.into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "sub".into(),
            parent_run_id: Some(parent),
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();

    // 命令行的归属只能是「任务」或「值班会话」二选一（迁移 0004 的 CHECK）。
    sqlx::query(
        "INSERT INTO kanban_node_commands
         (task_id, session_id, run_id, stage, node, source, command, original_command, cwd,
          started_at)
         VALUES (?, NULL, ?, 'init', 'execute', 'agent', 'ls', 'ls', '/tmp', ?)",
    )
    .bind(task_id)
    .bind(child)
    .bind(agentpipeline_core::storage::ts(store.now()))
    .execute(store.pool())
    .await
    .unwrap();

    store
        .insert_conversation(
            task_id,
            parent,
            Stage::Init,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "assistant", "content": "开工"}]),
            None,
            None,
            3,
            5,
            None,
        )
        .await
        .unwrap();

    // 节点内消息日志（迁移 0044，`.scratch/node-message-resume` 票 01）：它挂在 **run** 上
    // （外键 NO ACTION），故必须排在删 run 之前——漏了这一步就是那句 787。
    store
        .insert_node_message(
            task_id,
            child,
            Stage::Init,
            Node::Execute,
            "main",
            0,
            &agentpipeline_core::agent::client::Message::assistant(Some("开工".into()), Vec::new()),
            false,
        )
        .await
        .unwrap();

    store
        .insert_transition(
            task_id,
            "main",
            None,
            (Stage::Init, Node::Execute),
            TransitionTrigger::Start,
            None,
        )
        .await
        .unwrap();

    store
        .upsert_stage_output(task_id, Stage::Init, "design", "/tmp/design.md", None)
        .await
        .unwrap();

    store
        .note_attention(task_id, AttentionKind::TaskPending, store.now(), None)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO kanban_model_requests
         (run_id, session_id, task_id, agent_type, stage, node, attempt, seq, status, started_at)
         VALUES (?, NULL, ?, 'main', 'init', 'execute', 1, 1, 'ok', ?)",
    )
    .bind(child)
    .bind(task_id)
    .bind(agentpipeline_core::storage::ts(store.now()))
    .execute(store.pool())
    .await
    .unwrap();

    parent
}

/// 项目级伪阶段四件（决策 100 / 130⑦ / 329）：分析行 + 项目级 run + 项目级会话 +
/// 挂在该 run 上的**请求台账行**。
///
/// 分析行是线上那次 787 的**最后一道拦路者**（`kanban_project_analyses.project_id`），
/// 而项目分析是界面上一个正常动作、行从不清理——故它必须在用例里。
///
/// 请求台账行（`task_id` 为 NULL、只挂 `run_id`）是决策 329 修好之后**真实的形状**：
/// 它的 `task_id` 是空的，故按任务 id 扫的那半边扫不到它——不摆这一行，那条删除路径
/// 就是没人走过的。
///
/// 返回项目级 run 的 id：行删完就查不到了，验证残留只能靠先攥住它。
async fn seed_project_analysis(store: &Store, project_id: &str) -> i64 {
    store.create_analysis(project_id).await.unwrap();
    let run_id = store
        .insert_project_run(&NewProjectRun {
            project_id: project_id.into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: PSEUDO_PROJECT_ANALYSIS.into(),
        })
        .await
        .unwrap();
    store
        .insert_project_conversation(
            project_id,
            run_id,
            Stage::Init,
            Node::Execute,
            1,
            PSEUDO_PROJECT_ANALYSIS,
            &serde_json::json!([{"role": "assistant", "content": "摘要"}]),
            None,
            None,
            10,
            5,
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO kanban_model_requests
         (run_id, session_id, task_id, agent_type, stage, node, attempt, seq, status, started_at)
         VALUES (?, NULL, NULL, 'pseudo:project_analysis', 'init', 'execute', 1, 1, 'ok', ?)",
    )
    .bind(run_id)
    .bind(agentpipeline_core::storage::ts(store.now()))
    .execute(store.pool())
    .await
    .unwrap();
    run_id
}

/// 逐表单行读数（`?` 全部绑同一个 id）。
///
/// 逐表写死是有意的：将来新增一张挂 `project_id` / `task_id` 的表时，这里不跟着加
/// 就会漏检——而「哪张表忘了清」正是这个 bug 的形状。
async fn count_for(store: &Store, sql: &str, id: &str) -> i64 {
    let mut q = sqlx::query_scalar::<_, i64>(sql);
    for _ in 0..sql.matches('?').count() {
        q = q.bind(id);
    }
    q.fetch_one(store.pool()).await.unwrap()
}

/// 项目所属的行（按 `project_id`）。
const PROJECT_SCOPED: &[(&str, &str)] = &[
    (
        "kanban_projects",
        "SELECT COUNT(*) FROM kanban_projects WHERE id = ?",
    ),
    (
        "kanban_project_analyses",
        "SELECT COUNT(*) FROM kanban_project_analyses WHERE project_id = ?",
    ),
    (
        "kanban_node_runs(项目级)",
        "SELECT COUNT(*) FROM kanban_node_runs WHERE project_id = ?",
    ),
    (
        "kanban_node_conversations(项目级)",
        "SELECT COUNT(*) FROM kanban_node_conversations WHERE project_id = ?",
    ),
    (
        "kanban_tasks",
        "SELECT COUNT(*) FROM kanban_tasks WHERE project_id = ?",
    ),
];

/// 任务所属的行（按 `task_id`，含两个方向的依赖边）。
const TASK_SCOPED: &[(&str, &str)] = &[
    (
        "kanban_node_runs",
        "SELECT COUNT(*) FROM kanban_node_runs WHERE task_id = ?",
    ),
    (
        "kanban_node_cursors",
        "SELECT COUNT(*) FROM kanban_node_cursors WHERE task_id = ?",
    ),
    (
        "kanban_node_commands",
        "SELECT COUNT(*) FROM kanban_node_commands WHERE task_id = ?",
    ),
    (
        "kanban_node_conversations",
        "SELECT COUNT(*) FROM kanban_node_conversations WHERE task_id = ?",
    ),
    (
        // 节点内消息日志（迁移 0044）：挂在 run 上，故删除顺序必须排在删 run 之前。
        "kanban_node_messages",
        "SELECT COUNT(*) FROM kanban_node_messages WHERE task_id = ?",
    ),
    (
        "kanban_model_requests",
        "SELECT COUNT(*) FROM kanban_model_requests WHERE task_id = ?",
    ),
    (
        "kanban_foreman_attention",
        "SELECT COUNT(*) FROM kanban_foreman_attention WHERE task_id = ?",
    ),
    (
        "kanban_stage_outputs",
        "SELECT COUNT(*) FROM kanban_stage_outputs WHERE task_id = ?",
    ),
    (
        "kanban_transitions",
        "SELECT COUNT(*) FROM kanban_transitions WHERE task_id = ?",
    ),
    (
        "kanban_task_deps",
        "SELECT COUNT(*) FROM kanban_task_deps WHERE task_id = ? OR depends_on_id = ?",
    ),
];

/// 残留清单：空 = 删干净了。
///
/// `project_run_ids` 是**删除前攥住**的项目级 run id：那些 run 挂着的行（请求台账等）
/// 在 run 行删掉之后就再也查不到归属了，只能按 id 直查——按 `run_id IN (SELECT … WHERE
/// project_id = ?)` 反查会永远回 0，那是自欺。
async fn leftovers(
    store: &Store,
    project_id: &str,
    task_ids: &[String],
    project_run_ids: &[i64],
) -> Vec<String> {
    let mut out = Vec::new();
    for (label, sql) in PROJECT_SCOPED {
        let n = count_for(store, sql, project_id).await;
        if n > 0 {
            out.push(format!("{label}: {n} 行"));
        }
    }
    for (label, sql) in TASK_SCOPED {
        let mut n = 0;
        for t in task_ids {
            n += count_for(store, sql, t).await;
        }
        if n > 0 {
            out.push(format!("{label}: {n} 行"));
        }
    }
    for run_id in project_run_ids {
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM kanban_model_requests WHERE run_id = ?")
                .bind(run_id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        if n > 0 {
            out.push(format!(
                "kanban_model_requests(项目级 run {run_id}): {n} 行"
            ));
        }
    }
    out
}

#[tokio::test]
async fn delete_project_takes_its_whole_history_with_it() {
    // 验收：用过的项目（分析 + 项目级 run / 会话 / 请求台账 + 任务三件套）能删掉，
    // 且不留任何残留行——修改前这里就是那句 787。
    let (_home, store) = setup().await;
    let t1 = seed_used_task(&store, "t1", "p1").await;
    let t2 = seed_used_task(&store, "t2", "p1").await;
    assert!(t1 > 0 && t2 > 0);
    let project_run = seed_project_analysis(&store, "p1").await;
    for t in ["t1", "t2"] {
        store.archive_task(t).await.unwrap();
    }

    let deleted = store.delete_project("p1").await.unwrap();

    let mut deleted = deleted;
    deleted.sort();
    assert_eq!(
        deleted,
        vec!["t1".to_string(), "t2".to_string()],
        "应回报被删任务的 id"
    );
    assert_eq!(
        leftovers(&store, "p1", &deleted, &[project_run]).await,
        Vec::<String>::new(),
        "删完之后逐表都该是零行"
    );
    assert!(
        store.get_project("p1").await.unwrap().is_none(),
        "项目行该没了"
    );
}

#[tokio::test]
async fn delete_project_leaves_no_orphan_tasks() {
    // 验收：`kanban_tasks.project_id` 没有外键，父行先走不会报错、只会留孤儿——
    // 孤儿任务照样出现在看板上（列表是单表取的），故必须为零。
    let (_home, store) = setup().await;
    seed_used_task(&store, "t1", "p1").await;
    store.archive_task("t1").await.unwrap();

    store.delete_project("p1").await.unwrap();

    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kanban_tasks WHERE project_id NOT IN (SELECT id FROM kanban_projects)",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(orphans, 0, "不该有指向已删项目的孤儿任务");
}

#[tokio::test]
async fn delete_project_leaves_other_projects_alone() {
    // 验收：删除的边界是「这个项目」——别的项目一行不动；跨项目指向本项目的
    // 依赖边清掉（被依赖的任务没了，边就是死边），别项目自己的边留着。
    let (_home, store) = setup().await;
    let repo2 = _home.scratch_dir("proj2");
    seed_project(&store, "p2", "另一个", &repo2, "main")
        .await
        .unwrap();

    // p1 的 t1 依赖 p2 的 t9：跨项目边（被依赖的那一头必须先存在——依赖边有外键）。
    store
        .create_task(&NewTask::new("t9", "任务 t9", "p2"))
        .await
        .unwrap();
    let mut new_task = NewTask::new("t1", "任务 t1", "p1");
    new_task.depends_on = vec!["t9".into()];
    store.create_task(&new_task).await.unwrap();
    // p2 内部的边（t8 → t9）：与本次删除无关，必须留着。
    let mut inner = NewTask::new("t8", "任务 t8", "p2");
    inner.depends_on = vec!["t9".into()];
    store.create_task(&inner).await.unwrap();
    store.archive_task("t1").await.unwrap();

    store.delete_project("p1").await.unwrap();

    assert!(
        store.get_project("p2").await.unwrap().is_some(),
        "p2 项目行不该动"
    );
    assert!(store.get_task("t9").await.is_ok(), "p2 的任务不该动");
    assert!(store.get_task("t8").await.is_ok(), "p2 的任务不该动");
    assert_eq!(
        store.load_live_cursors("t9").await.unwrap().len(),
        1,
        "p2 任务的游标不该动"
    );

    let dangling: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kanban_task_deps
         WHERE task_id = 't1' OR depends_on_id = 't1'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(dangling, 0, "指向已删任务的依赖边该被清掉");

    let kept: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kanban_task_deps WHERE task_id = 't8' AND depends_on_id = 't9'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(kept, 1, "p2 自己的依赖边不该被牵连");
}

#[tokio::test]
async fn delete_project_still_refuses_while_a_task_is_active() {
    // 验收：决策 101 的闸门是级联删除的联锁——活跃任务在，整件事不发生
    // （不是「删一半」）。拒绝之后闸门放开，同一份数据要能一次删净。
    let (_home, store) = setup().await;
    seed_used_task(&store, "t1", "p1").await;
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();
    let project_run = seed_project_analysis(&store, "p1").await;

    let err = store.delete_project("p1").await.unwrap_err();
    assert!(
        matches!(err, agentpipeline_core::Error::Conflict(_)),
        "活跃任务该被拒绝，实得：{err:?}"
    );
    assert!(
        store.get_project("p1").await.unwrap().is_some(),
        "拒绝之后项目行必须还在"
    );
    assert!(
        !leftovers(&store, "p1", &["t1".to_string()], &[project_run])
            .await
            .is_empty(),
        "拒绝之后一行都不该少"
    );

    // 任务收工（不再活跃）之后，同一份数据一次删净。
    store.archive_task("t1").await.unwrap();
    let deleted = store.delete_project("p1").await.unwrap();
    assert_eq!(deleted, vec!["t1".to_string()]);
    assert_eq!(
        leftovers(&store, "p1", &deleted, &[project_run]).await,
        Vec::<String>::new()
    );
}
