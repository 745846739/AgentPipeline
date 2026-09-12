//! E2E 场景共享 harness（testing.md §8）：FakeAgent 驱动完整流水线 +
//! testkit fixture + 临时 home + 手动 tick / 假时钟。
//!
//! FakeAgent 只替换 LLM 响应流；工具层、git、命令记录全部真实执行（决策 148）。
#![allow(dead_code)]

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    AcceptanceCriterion, Approval, ArchitectExecuteMetadata, CodeChanges, DevelopDesignMetadata,
    DiffStats, MergeResult, MergeStatus, Node, NodeCursor, Project, ReviewResult, ScenarioPriority,
    Stage, TestDesignMetadata, TestResult, TestScenario, ValidateInputMetadata,
    ValidateOutputMetadata,
};
use testkit::{FakeAgent, ManualClock, RecordingKiller, Repo, Script, SseRecorder, TestHome};

/// 一套完整流水线装置（每个测试独占临时 home / 仓库 / DB）。
pub struct Flow {
    pub home: TestHome,
    pub repo: Repo,
    pub store: Store,
    pub clock: ManualClock,
    pub killer: RecordingKiller,
    pub sse: SseRecorder,
    pub agent: FakeAgent,
    pub executor: Arc<Executor>,
    pub resumes: Arc<AtomicUsize>,
    pub settings: Settings,
}

impl Flow {
    pub async fn new() -> Self {
        Flow::with_settings(Settings::default()).await
    }

    pub async fn with_settings(settings: Settings) -> Self {
        let repo = Repo::clean().unwrap();
        Flow::with_repo_settings(repo, settings).await
    }

    pub async fn with_repo(repo: Repo) -> Self {
        Flow::with_repo_settings(repo, Settings::default()).await
    }

    pub async fn with_repo_settings(repo: Repo, settings: Settings) -> Self {
        let home = TestHome::new().unwrap();
        let clock = ManualClock::fixed();
        let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        // test_framework 配原始命令 `true`：系统闸门零噪声通过（fixture 不是可构建工程）
        let project = Project {
            id: "p1".into(),
            name: "示例".into(),
            local_path: repo.path().display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: Some("true".into()),
            lint_command: None,
            agents_md_path: None,
            created_at: store.now(),
        };
        store.create_project(&project).await.unwrap();

        let sse = SseRecorder::new();
        let killer = RecordingKiller::new();
        let agent = FakeAgent::new(Script::new());
        let executor = Arc::new(Executor::new(
            store.clone(),
            settings.clone(),
            Arc::new(sse.clone()),
            Arc::new(agent.clone()),
            Arc::new(killer.clone()),
        ));
        Flow {
            home,
            repo,
            store,
            clock,
            killer,
            sse,
            agent,
            executor,
            resumes: Arc::new(AtomicUsize::new(0)),
            settings,
        }
    }

    pub fn scheduler(&self) -> KanbanScheduler {
        self.scheduler_with(self.settings.clone())
    }

    pub fn scheduler_with(&self, settings: Settings) -> KanbanScheduler {
        let resumes = self.resumes.clone();
        KanbanScheduler::new(
            self.store.clone(),
            settings,
            Arc::new(self.clock.clone()),
            Arc::new(self.killer.clone()),
            Arc::new(self.sse.clone()),
            Arc::new(move |_t: &str| {
                resumes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        )
    }

    /// 准入并断言恰好放行该任务。
    pub async fn admit(&self, task_id: &str) {
        let report = self.scheduler().tick().await.unwrap();
        assert_eq!(report.admitted, vec![task_id.to_string()]);
    }

    pub async fn live_cursors(&self, task_id: &str) -> Vec<NodeCursor> {
        self.store.load_live_cursors(task_id).await.unwrap()
    }

    pub async fn live_cursor(&self, task_id: &str, branch: &str) -> NodeCursor {
        self.store
            .load_live_cursors(task_id)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.branch == branch)
            .unwrap_or_else(|| panic!("游标 {branch} 应存在"))
    }

    pub async fn sole_cursor(&self, task_id: &str) -> NodeCursor {
        let live = self.live_cursors(task_id).await;
        assert_eq!(live.len(), 1, "应只有一条活跃游标：{live:?}");
        live.into_iter().next().unwrap()
    }

    /// develop / review / test 的最小闭环（真写代码 + 真提交）。
    pub fn implementation_script(&self, script: &mut Script, task_id: &str) {
        implementation_ok(script, task_id);
    }

    /// 直接推出一个 merge_result 行（阶段 A 后置状态构造用）。
    pub async fn put_merge(&self, task_id: &str, merge: MergeResult) {
        self.store
            .upsert_merge_result(task_id, &merge.diff_path, &merge)
            .await
            .unwrap();
    }

    /// 造 worktree（跳过 init 节点时用）。
    pub async fn init_worktree(&self, task_id: &str) -> std::path::PathBuf {
        let worktree = self.home.home().worktree_path(task_id);
        agentpipeline_core::git::Git
            .init_worktree(self.repo.path(), task_id, &worktree, "main")
            .await
            .unwrap();
        worktree
    }
}

/// merge_result 的文档必填字段齐全构造（§4.2）。
pub fn merge_row(diff_path: &str, base_commit: &str) -> MergeResult {
    MergeResult {
        diff_path: diff_path.into(),
        diff_stats: DiffStats {
            files_changed: 1,
            insertions: 0,
            deletions: 0,
            file_details: Vec::new(),
        },
        base_commit: base_commit.into(),
        gate: None,
        gate_failure_kind: None,
        gate_failures: 0,
        gate_failure_output: None,
        conflict_files: Vec::new(),
        approval: Approval::None,
        status: MergeStatus::PendingApproval,
    }
}

// ─────────────────────────── 脚本片段 ───────────────────────────

/// architect-design 三节点正常通过（design.md 带 AC-1，供 design_refs 引用）。
pub fn architect_ok(script: &mut Script) {
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/lib.rs".into()],
            new_symbols: vec![],
            acceptance_criteria: vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "能登录".into(),
            }],
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

pub fn dev_design_ok(script: &mut Script) {
    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::DevelopDesign, Node::Execute)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit(&DevelopDesignMetadata {
            readiness: true,
            dev_doc_path: Some("dev-plan.md".into()),
            ..Default::default()
        });
    script
        .for_node(Stage::DevelopDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

pub fn test_design_ok(script: &mut Script) {
    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            blockers: vec![],
            test_scenarios_path: Some("test-scenarios.md".into()),
            test_scenarios: vec![TestScenario {
                id: "S-1".into(),
                name: "登录成功".into(),
                description: "登录".into(),
                preconditions: vec![],
                steps: vec![],
                expected_result: "成功".into(),
                priority: ScenarioPriority::High,
                design_refs: vec!["AC-1".into()],
            }],
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

/// 设计三分支 + 双并行分支全部就绪。
pub fn design_ok(script: &mut Script) {
    architect_ok(script);
    dev_design_ok(script);
    test_design_ok(script);
}

/// develop / review / test 的通过脚本（真写代码 + 真提交）。
pub fn implementation_ok(script: &mut Script, task_id: &str) {
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(&format!(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task {task_id}'"
        ))
        .submit(&CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
        });
    script
        .for_node(Stage::Review, Node::Execute)
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    script
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 测试报告\n全部通过\n")
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
}

/// 全流程通过脚本（到 merge 阶段 A 为止）。
pub fn full_pass_script(script: &mut Script, task_id: &str) {
    design_ok(script);
    implementation_ok(script, task_id);
}
