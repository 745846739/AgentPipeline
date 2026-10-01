//! L2 集成：修复的「当场生效」（决策 358 / 票 13）。
//!
//! 测的是**真 git + 真台账**：存量真的被清了（且清前落了账）、补丁真的落在任务
//! worktree、任务工作区的闸门真的重跑了一遍、`[repair]` commit 真的只含修复、
//! 托管的止损真的拦得住（没开 / 次数满 / 人按住都回落「等合入」）。
//! resume 的执行者用替身（与票 08 的托管用例同一姿态）：core 这边要证的是
//! 「闸放行了、执行者被叫到了、账留下了」——resume 状态机本身另有钉子。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentpipeline_core::agent::client::LlmClient;
use agentpipeline_core::agent::tools::{StewardActionRunner, ToolOutcome};
use agentpipeline_core::clock::Clock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::{ForemanRunner, FOREMAN_STAGE_KEY};
use agentpipeline_core::pipeline::repair::{new_repair_id, start_repair, REPAIR_COMMIT_MARK};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    EnvMode, Node, PendingKind, PendingReason, Stage, StageConfig, Stewardship,
};
use testkit::{FakeAgent, ManualClock, Repo, Script, SseRecorder, TestHome};

// ─────────────── 脚手架 ───────────────

struct Harness {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
}

impl Harness {
    async fn new() -> Self {
        let home = TestHome::new().unwrap();
        let clock = ManualClock::fixed();
        let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        Harness {
            _home: home,
            store,
            clock,
        }
    }

    /// 真 git 仓接成 p1；闸门命令原样返回——`"true"` 恒过、可定向构造恒过/恒fail。
    async fn git_project(&self, repo: &Repo, test_framework: &str) {
        let project = agentpipeline_core::types::Project {
            id: "p1".into(),
            name: "示例项目".into(),
            local_path: repo.path().display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: Some(test_framework.into()),
            lint_command: None,
            agents_md_path: None,
            created_at: self.store.now(),
        };
        self.store.create_project(&project).await.unwrap();
        // repair 归环境层：`ask` 档会转提议，这里要测的是执行本身——显式配 auto。
        self.store
            .upsert_stage_config(&StageConfig {
                stage: FOREMAN_STAGE_KEY.to_string(),
                env_mode: Some(EnvMode::Auto),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    fn runner_with_steward(&self, script: Script, steward: Arc<RecordingSteward>) -> ForemanRunner {
        ForemanRunner::new(
            self.store.clone(),
            Settings::default(),
            self._home.home().clone(),
            Arc::new(FakeAgent::new(script)) as Arc<dyn LlmClient>,
            Arc::new(SseRecorder::new()),
        )
        .with_steward_actions(steward)
    }
}

/// 托管动作的执行替身：记下每次被叫时的参数 JSON——闸放行、落点、after_repair 标记
/// 都从这里断言。执行本身是桩（resume 状态机由 app 层那份实现负责，core 不重复测）。
struct RecordingSteward {
    calls: Mutex<Vec<serde_json::Value>>,
}

impl RecordingSteward {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }
}

impl StewardActionRunner for RecordingSteward {
    fn run(
        &self,
        call: agentpipeline_core::agent::client::ToolCall,
        _ctx: agentpipeline_core::agent::tools::ToolCallContext,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<ToolOutcome>> {
        let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            return Box::pin(async move { Ok(ToolOutcome::ok("替身看不懂的参数")) });
        };
        self.calls.lock().unwrap().push(args);
        Box::pin(async move { Ok(ToolOutcome::ok("已自动放行（托管，测试替身）")) })
    }
}

/// 在某个目录里跑 git（任务 worktree 的提交不经过 [testkit::Repo]，它锚在项目仓上）。
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git 应当可执行");
    assert!(
        out.status.success(),
        "git {:?} 失败：{}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 任务的 worktree：照生产的形状——真 worktree（home 下）、任务行指向它、
/// 已提交的进度 + 未提交的半成品（存量）。
async fn task_worktree(h: &Harness, repo: &Repo) -> PathBuf {
    let wt = h._home.home().worktrees_dir().join("t1");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "task/t1",
        &wt.to_string_lossy(),
        "main",
    ]);
    // 已提交的进度：reset --hard 到 HEAD 动不了它
    std::fs::write(wt.join("feature.txt"), "任务已写好的部分\n").unwrap();
    git_in(&wt, &["add", "-A"]);
    git_in(&wt, &["commit", "-m", "任务的进度（已提交）"]);
    // 未提交的半成品：决策 358② 说的「上一轮失败尝试的过时残留」
    std::fs::write(wt.join("leftover.txt"), "上一轮没写完的东西\n").unwrap();
    h.store
        .set_task_worktree("t1", &wt.to_string_lossy(), "task/t1")
        .await
        .unwrap();
    wt
}

/// 把任务推成 pending（与 foreman.rs 的 park_task 同一份两步走）。
async fn park_task(store: &Store, task_id: &str, kind: PendingKind, message: &str) {
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

/// 一次完整的当场生效现场：项目 + 任务 + worktree（进度 + 存量）+ pending + 托管。
/// 返回任务 worktree 路径与 pending 阶段名。
async fn deliver_fixture(
    h: &Harness,
    repo: &Repo,
    stewardship: Option<Stewardship>,
    park: PendingKind,
) -> (PathBuf, String) {
    h.git_project(repo, "true").await;
    testkit::seed_task(&h.store, "t1", "p1").await.unwrap();
    let wt = task_worktree(h, repo).await;
    park_task(&h.store, "t1", park, "重试耗尽，等你拍板").await;
    if let Some(s) = stewardship {
        h.store.set_stewardship("t1", Some(&s)).await.unwrap();
    }
    let stage = h.store.load_live_cursors("t1").await.unwrap()[0]
        .pending_reason
        .as_ref()
        .unwrap()
        .stage
        .as_str()
        .to_string();
    (wt, stage)
}

/// 修复现场：start_repair 拉起 worktree 并写好修复（deliver 会跑闸门并 commit）。
async fn prepared_repair(h: &Harness, repo: &Repo, fix: &str) -> String {
    let rid = new_repair_id();
    let session = start_repair(
        h._home.home(),
        repo.path(),
        "main",
        &rid,
        &h.store.create_foreman_session("").await.unwrap().id,
    )
    .await
    .unwrap();
    std::fs::write(session.worktree.join("fix.txt"), fix).unwrap();
    rid
}

fn deliver_script(project_id: &str, repair_id: &str, task_id: &str) -> Script {
    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({
            "action": "deliver",
            "project_id": project_id,
            "repair_id": repair_id,
            "task_id": task_id,
            "conclusion": "修好了缺的那块",
        }),
    );
    script.for_foreman().text("说清走向。");
    script
}

// ─────────────── 用例 ───────────────

/// 幸福路：存量被清（且清前落账）、补丁落进任务 worktree、任务闸门过了、
/// `[repair]` commit 只含修复、托管 resume 被叫到、账留了两处。
#[tokio::test]
async fn delivered_clears_leftovers_lands_the_patch_and_auto_resumes() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let stewardship = Stewardship::enabled_now(h.clock.now());
    let (wt, stage) =
        deliver_fixture(&h, &repo, Some(stewardship), PendingKind::RetryExhausted).await;
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    // 回执：当场生效完成
    assert!(
        turn.traces[0].result.contains("当场生效完成"),
        "{}",
        turn.traces[0].result
    );
    // 托管 resume 被叫到，且形状是「goto 到卡住阶段的入口 + after_repair 标记」
    {
        let calls = steward.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{:?}", *calls);
        let resume = &calls[0];
        assert_eq!(resume["resume_action"], "goto");
        assert_eq!(resume["target_stage"], stage.as_str());
        assert_eq!(resume["after_repair"], rid.as_str());
    }

    // 任务 worktree：存量没了、已提交进度还在、修复进来了、commit 只含修复
    assert!(!wt.join("leftover.txt").exists(), "存量应被清掉");
    assert!(wt.join("feature.txt").exists(), "已提交进度不动");
    assert!(wt.join("fix.txt").exists(), "补丁应落进任务工作区");
    let head_msg = git_in(&wt, &["log", "-1", "--format=%s"]);
    assert!(
        head_msg.contains(REPAIR_COMMIT_MARK),
        "任务分支顶端应是 [repair] commit：{head_msg}"
    );
    let changed = git_in(&wt, &["show", "--stat", "--format=", "HEAD"]);
    assert!(changed.contains("fix.txt"), "{changed}");
    assert!(
        !changed.contains("leftover.txt"),
        "存量不该被卷进修复 commit"
    );

    // 账落了两处：任务行上的计数与指纹、班次里一条【托管】行
    let s = h.store.get_task("t1").await.unwrap().stewardship.unwrap();
    assert_eq!(s.auto_resumes, 1);
    assert!(s.last_fingerprint.is_some());
    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    assert!(
        messages.iter().any(|m| m.content.starts_with("【托管】")),
        "每次自动动手都要在班次里留账"
    );

    // 清存量落了账（system 命令、任务归属、清前脏态读数）
    let commands = h.store.list_commands("t1", None, None).await.unwrap();
    let clean = commands
        .iter()
        .find(|c| c.command.contains("reset --hard"))
        .expect("先清后落的清要落命令台账");
    assert!(
        clean
            .stdout_preview
            .as_deref()
            .unwrap_or("")
            .contains("清前脏态"),
        "{:?}",
        clean.stdout_preview
    );

    // 幸福路不落「等合入」提议——修复合入那颗钮没有存在必要
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());
}

/// 托管没开：工作区一个字不动，原样回落「等合入」（提议照落）。
#[tokio::test]
async fn without_stewardship_it_falls_back_to_the_merge_proposal() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let (wt, _stage) = deliver_fixture(&h, &repo, None, PendingKind::RetryExhausted).await;
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("当场生效没走成"),
        "{}",
        turn.traces[0].result
    );
    assert!(steward.calls.lock().unwrap().is_empty(), "托管没开：不动手");
    assert!(wt.join("leftover.txt").exists(), "回落不该碰工作区");
    assert!(!wt.join("fix.txt").exists(), "回落不落补丁");
    let pending = h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "回落 = 今天 finish 的形状：{pending:?}");
    assert_eq!(pending[0].tool, "repair");
}

/// 次数满（210⑨ 的 N=2）：同一个回落。
#[tokio::test]
async fn falls_back_when_the_stewardship_quota_is_exhausted() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let exhausted = Stewardship {
        enabled: true,
        auto_resumes: 2,
        ..Default::default()
    };
    let (wt, _) = deliver_fixture(&h, &repo, Some(exhausted), PendingKind::RetryExhausted).await;
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("当场生效没走成"),
        "{}",
        turn.traces[0].result
    );
    assert!(steward.calls.lock().unwrap().is_empty());
    assert!(!wt.join("fix.txt").exists());
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// 人按住的暂停（决策 276）：托管不代劳，同一个回落。
#[tokio::test]
async fn falls_back_when_the_human_holds_the_task() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let stewardship = Stewardship::enabled_now(h.clock.now());
    let (wt, _) = deliver_fixture(&h, &repo, Some(stewardship), PendingKind::UserPaused).await;
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("当场生效没走成"),
        "{}",
        turn.traces[0].result
    );
    assert!(steward.calls.lock().unwrap().is_empty());
    assert!(!wt.join("fix.txt").exists());
}

/// 任务工作区里的组合闸门没过：补丁撤回、不 resume、不提议——照票 11.1
/// 「没过就播报失败、不出 diff」。
#[tokio::test]
async fn restores_the_worktree_when_the_task_worktree_gate_fails() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    // 闸门命令：task-only.txt 在场即失败——修复 worktree（从 main 分出）没有它，任务 worktree 有。
    h.git_project(&repo, "test ! -f task-only.txt").await;
    testkit::seed_task(&h.store, "t1", "p1").await.unwrap();
    let wt = task_worktree(&h, &repo).await;
    std::fs::write(wt.join("task-only.txt"), "任务自己的文件\n").unwrap();
    git_in(&wt, &["add", "-A"]);
    git_in(&wt, &["commit", "-m", "任务第二笔进度"]);
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;
    h.store
        .set_stewardship("t1", Some(&Stewardship::enabled_now(h.clock.now())))
        .await
        .unwrap();
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("闸门没过"),
        "{}",
        turn.traces[0].result
    );
    assert!(
        steward.calls.lock().unwrap().is_empty(),
        "闸门不过：不 resume"
    );
    assert!(!wt.join("fix.txt").exists(), "补丁要撤回");
    assert!(wt.join("task-only.txt").exists(), "已提交进度不动");
    let head_msg = git_in(&wt, &["log", "-1", "--format=%s"]);
    assert!(
        !head_msg.contains(REPAIR_COMMIT_MARK),
        "闸门不该有修复 commit：{head_msg}"
    );
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());
}

/// 补丁与任务工作区冲突（同一批行被两边都改过）：落不进就回落，工作区回原状。
#[tokio::test]
async fn falls_back_when_the_patch_conflicts_with_task_work() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let (wt, _) = deliver_fixture(
        &h,
        &repo,
        Some(Stewardship::enabled_now(h.clock.now())),
        PendingKind::RetryExhausted,
    )
    .await;
    // 任务分支把 src/lib.rs 那一行整个改掉；修复（基于 main 的它）也改这一行——落不进
    std::fs::write(
        wt.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a * 2 }\n",
    )
    .unwrap();
    git_in(&wt, &["add", "-A"]);
    git_in(&wt, &["commit", "-m", "任务重写了 lib"]);
    let rid = new_repair_id();
    let session = start_repair(
        h._home.home(),
        repo.path(),
        "main",
        &rid,
        &h.store.create_foreman_session("").await.unwrap().id,
    )
    .await
    .unwrap();
    std::fs::write(
        session.worktree.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b } // 修复：补回正确实现\n",
    )
    .unwrap();

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("当场生效没走成"),
        "{}",
        turn.traces[0].result
    );
    assert!(steward.calls.lock().unwrap().is_empty());
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .len(),
        1,
        "冲突回落：修复走等合入"
    );
}

/// 闸门在修复 worktree 就没过：什么都不落（票 11.1 的字面），deliver 与 finish 同门。
#[tokio::test]
async fn a_repair_gate_failure_delivers_nothing() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    h.git_project(&repo, "false").await;
    testkit::seed_task(&h.store, "t1", "p1").await.unwrap();
    let wt = task_worktree(&h, &repo).await;
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;
    h.store
        .set_stewardship("t1", Some(&Stewardship::enabled_now(h.clock.now())))
        .await
        .unwrap();
    let rid = prepared_repair(&h, &repo, "修复的正文\n").await;

    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid, "t1"), steward.clone());
    let turn = runner.say(None, "修好了没有").await.unwrap();

    assert!(
        turn.traces[0].result.contains("闸门没过"),
        "{}",
        turn.traces[0].result
    );
    assert!(steward.calls.lock().unwrap().is_empty());
    assert!(!wt.join("fix.txt").exists(), "闸门不过不落补丁");
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());
}

/// 编译期守卫：`Stage` / `Node` 的 as_str 在落点推导里用到（占位引用，防误删导出）。
#[allow(dead_code)]
fn _stage_node_witness(_s: Stage, _n: Node) {}

/// 同一态势指纹不重复动手（决策 210⑨）：第一次当场生效已把指纹记在任务上，
/// 第二次 deliver（局势没变）在预检就被拦下——工作区不动、回落「等合入」。
#[tokio::test]
async fn a_second_delivery_on_the_same_situation_stands_down() {
    let h = Harness::new().await;
    let repo = Repo::clean().unwrap();
    let (wt, _) = deliver_fixture(
        &h,
        &repo,
        Some(Stewardship::enabled_now(h.clock.now())),
        PendingKind::RetryExhausted,
    )
    .await;

    // 第一次：当场生效成功，指纹落库
    let rid1 = prepared_repair(&h, &repo, "第一版修复\n").await;
    let steward = RecordingSteward::new();
    let runner = h.runner_with_steward(deliver_script("p1", &rid1, "t1"), steward.clone());
    let turn1 = runner.say(None, "修好了没有").await.unwrap();
    assert!(turn1.traces[0].result.contains("当场生效完成"));
    assert_eq!(steward.calls.lock().unwrap().len(), 1);

    // 第二次（局势没动）：预备好了另一版修复，预检就该拦下
    let rid2 = prepared_repair(&h, &repo, "第二版修复\n").await;
    let mut script = deliver_script("p1", &rid2, "t1");
    script.for_foreman().text("说清走向。");
    let runner2 = h.runner_with_steward(script, steward.clone());
    let turn2 = runner2.say(None, "又修了一版").await.unwrap();

    assert!(
        turn2.traces[0].result.contains("当场生效没走成"),
        "{}",
        turn2.traces[0].result
    );
    assert!(
        turn2.traces[0].result.contains("托管不许可"),
        "{}",
        turn2.traces[0].result
    );
    assert_eq!(
        steward.calls.lock().unwrap().len(),
        1,
        "同一态势：执行者不被叫第二次"
    );
    assert!(
        wt.join("fix.txt").exists(),
        "第一次的补丁还在（未被第二版碰过）"
    );
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&turn2.session.id)
            .await
            .unwrap()
            .len(),
        1,
        "回落照旧落「等合入」提议"
    );
}
