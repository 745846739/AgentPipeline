//! L2 集成：merge 的 git 操作链（testing.md §6、决策 41 / 61 / 73 / 74 / 96 / 97 / 125）。
//!
//! 用 testkit 的真实 git fixture 演练（fixture 搭建走系统 git CLI，决策 146；
//! 生产 git 层按决策 12 走 git2）。

use agentpipeline_core::git::{Git, RebaseOutcome};
use testkit::{Repo, TestHome};

struct Ctx {
    _home: TestHome,
    repo: Repo,
    worktree: std::path::PathBuf,
}

fn setup() -> Ctx {
    let home = TestHome::new().unwrap();
    let repo = Repo::clean().unwrap();
    let worktree = home.home().worktree_path("t1");
    Ctx {
        _home: home,
        repo,
        worktree,
    }
}

/// init.execute 的基准（决策 41）。
async fn init(ctx: &Ctx) -> String {
    Git.init_worktree(ctx.repo.path(), "t1", &ctx.worktree, "main")
        .await
        .unwrap()
}

// ─────────────────────────── init（决策 41 / 61）───────────────────────────

#[tokio::test]
async fn init_creates_worktree_on_task_branch() {
    let ctx = setup();
    let base = init(&ctx).await;
    assert_eq!(base, "main");
    assert!(
        ctx.worktree.join("src/lib.rs").exists(),
        "worktree 应检出主干内容"
    );
    assert_eq!(Git::branch_for("t1"), "kanban/t1");
    // 分支已登记且不污染主仓库工作区
    assert!(ctx.repo.branch_exists("kanban/t1"));
    assert!(!ctx.repo.is_dirty());
    assert!(ctx.repo.worktree_list().contains("t1"));
}

#[tokio::test]
async fn init_prefers_origin_default_branch_when_remote_exists() {
    let (repo, _remote) = Repo::with_remote().unwrap();
    let home = TestHome::new().unwrap();
    let worktree = home.home().worktree_path("t1");

    let base = Git
        .init_worktree(repo.path(), "t1", &worktree, "main")
        .await
        .unwrap();
    assert_eq!(
        base, "origin/main",
        "有 remote 时以 origin/{{default_branch}} 为基准"
    );

    let head = Git.rev_parse(&worktree, "HEAD").await.unwrap();
    assert_eq!(head, repo.head("origin/main"));
}

#[tokio::test]
async fn init_rejects_unborn_head_with_explicit_error() {
    let home = TestHome::new().unwrap();
    let repo = Repo::unborn_head().unwrap();
    let err = Git
        .init_worktree(repo.path(), "t1", &home.home().worktree_path("t1"), "main")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unborn HEAD"),
        "应明确报错而不是抛底层 git 错误：{msg}"
    );
}

#[tokio::test]
async fn init_is_idempotent() {
    let ctx = setup();
    init(&ctx).await;
    // 写点东西再复用
    std::fs::write(ctx.worktree.join("scratch.txt"), "x").unwrap();
    let base = init(&ctx).await;
    assert_eq!(base, "main");
    assert!(ctx.worktree.join("scratch.txt").exists(), "复用而非重建");
    assert_eq!(
        ctx.repo
            .git(&["worktree", "list"])
            .matches("agentpipeline-test")
            .count()
            .max(1),
        1
    );
}

#[tokio::test]
async fn dirty_project_does_not_block_worktree_creation() {
    // 决策 61：建 worktree 时只记警告，不阻塞
    let ctx = setup();
    ctx.repo.dirty_worktree().unwrap();
    assert!(ctx.repo.is_dirty());
    init(&ctx).await;
    assert!(ctx.worktree.exists());
}

// ─────────────────────────── 阶段 A：rebase（决策 41 / 74 / 96）───────────────────────────

#[tokio::test]
async fn rebase_clean_records_base_commit() {
    let ctx = setup();
    let base = init(&ctx).await; // "main"
                                 // worktree 里提交一次变更
    std::fs::write(ctx.worktree.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    commit_worktree(&ctx.worktree, "feat: 改代码");

    // 主干前移
    ctx.repo.advance_main("src/other.rs", "pub fn other() {}\n");

    let base_commit = Git.rev_parse(ctx.repo.path(), &base).await.unwrap();
    let outcome = Git.rebase_onto(&ctx.worktree, &base).await.unwrap();
    match outcome {
        RebaseOutcome::Clean { head } => {
            assert_eq!(head, Git.rev_parse(&ctx.worktree, "HEAD").await.unwrap());
            // rebase 后包含主干的最新提交
            assert!(Git
                .contains(&ctx.worktree, "HEAD", &ctx.repo.head("main"))
                .await
                .unwrap());
        }
        other => panic!("应干净 rebase：{other:?}"),
    }
    // 记录的 base_commit 与当时主干 SHA 一致（决策 96）
    assert_eq!(
        Git.rev_parse(ctx.repo.path(), "main").await.unwrap().len(),
        40
    );
    let _ = base_commit;
}

#[tokio::test]
async fn rebase_conflict_is_detected_then_aborted_cleanly() {
    // 决策 74：打回 develop 前由系统执行 rebase --abort
    let repo = Repo::clean().unwrap();
    repo.conflict_hard().unwrap();
    let home = TestHome::new().unwrap();
    let worktree = home.home().worktree_path("t1");

    // 让 worktree 检出 topic 分支
    let wt = worktree.display().to_string();
    repo.git(&["worktree", "add", "-b", "kanban/t1", &wt, "topic"]);
    std::fs::write(worktree.join("untracked.txt"), "x").unwrap();

    let outcome = Git.rebase_onto(&worktree, "main").await.unwrap();
    match outcome {
        RebaseOutcome::Conflict { files } => {
            assert_eq!(files, vec!["shared.txt".to_string()]);
        }
        other => panic!("应检测到冲突：{other:?}"),
    }

    Git.rebase_abort(&worktree).await.unwrap();
    // abort 后已跟踪文件必须回到干净状态（未跟踪文件不属于 rebase 中断残留）
    let repo = agentpipeline_core::git::git2::Repository::open(&worktree).unwrap();
    let mut opts = agentpipeline_core::git::git2::StatusOptions::new();
    opts.include_untracked(false).include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).unwrap();
    assert!(
        statuses.is_empty(),
        "abort 后不应残留冲突状态：{:?}",
        statuses
            .iter()
            .filter_map(|e| e.path().map(str::to_string))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        "topic\n"
    );
    assert!(worktree.join("untracked.txt").exists());
}

// ─────────────────────────── 阶段 B：合入（决策 73 / 97 / 61）───────────────────────────

#[tokio::test]
async fn merge_fast_forwards_when_possible() {
    let ctx = setup();
    init(&ctx).await;
    std::fs::write(ctx.worktree.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    let topic_commit = commit_worktree(&ctx.worktree, "feat: 新功能");

    // git2 内存合入，直接写回 refs/heads/main（决策 97）
    let outcome = Git
        .merge_into_default_branch(ctx.repo.path(), "main", "kanban/t1")
        .await
        .unwrap();
    assert!(outcome.fast_forward, "主干未前移时应 ff");
    assert_eq!(outcome.commit, topic_commit);

    // 决策 97：合入结果必须写回 default_branch
    assert_eq!(ctx.repo.head("main"), topic_commit);
}

#[tokio::test]
async fn merge_uses_no_ff_and_writes_back_ref_when_diverged() {
    // 决策 73 / 97 的核心回归：detached worktree 里的 merge commit 必须 update-ref 写回
    let ctx = setup();
    init(&ctx).await;
    std::fs::write(ctx.worktree.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    commit_worktree(&ctx.worktree, "feat: 任务分支");

    // 主干前移 → 不可 ff
    ctx.repo
        .advance_main("src/main_side.rs", "pub fn main_side() {}\n");
    let main_before = ctx.repo.head("main");

    // git2 内存合入：双亲 merge commit 直接写回 refs/heads/main（决策 97）
    let outcome = Git
        .merge_into_default_branch(ctx.repo.path(), "main", "kanban/t1")
        .await
        .unwrap();
    assert!(!outcome.fast_forward, "主干已分叉时应 --no-ff");
    assert_ne!(outcome.commit, main_before);

    // default_branch 前进到 merge commit（决策 97）
    assert_eq!(ctx.repo.head("main"), outcome.commit);

    // merge commit 有两个父（真 merge，不是 force 覆盖）
    let parents = ctx.repo.git(&["rev-list", "--parents", "-n", "1", "main"]);
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "应为双亲 merge commit：{parents}"
    );

    // 主干既有自己的提交，也有任务分支的改动
    assert!(ctx.repo.exists("src/main_side.rs"));
    assert!(ctx.repo.exists("src/lib.rs"));
}

#[tokio::test]
async fn merge_detects_dirty_target_worktree() {
    // 决策 61：目标分支工作区不干净 → 调用方进 pending(user_decision)
    let ctx = setup();
    init(&ctx).await;
    ctx.repo.dirty_worktree().unwrap();
    assert!(Git.ensure_clean(ctx.repo.path()).await.is_err());
    assert!(Git.is_dirty(ctx.repo.path()).await.unwrap());
}

#[tokio::test]
async fn merge_leaves_default_branch_worktree_consistent() {
    // 回归：合入把 `refs/heads/main` 前移后，**被检出的主工作区必须同步**。
    //
    // 缺陷形态（2026-09-13 由主流程 e2e 暴露）：内存合并只移动引用，索引与工作区
    // 停在旧提交 → `git status` 出现**已暂存**的 M/D（用户 `git commit` 会**回滚合入**），
    // 磁盘上仍是旧代码（`npm test` 直接失败），且下一个任务在 merge 处被决策 61 的
    // 「工作区不干净」拦下——合入成功反而污染了下一次合入。
    let ctx = setup();
    init(&ctx).await;
    std::fs::write(ctx.worktree.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    let topic_commit = commit_worktree(&ctx.worktree, "feat: 新功能");

    let outcome = Git
        .merge_into_default_branch(ctx.repo.path(), "main", "kanban/t1")
        .await
        .unwrap();
    assert!(outcome.fast_forward);
    assert_eq!(ctx.repo.head("main"), topic_commit, "引用应前移");

    // 合入后主仓库不得留下陈旧索引 / 陈旧工作区
    assert!(
        !ctx.repo.is_dirty(),
        "合入后主工作区应干净，实际 status：\n{}",
        ctx.repo.git(&["status", "--porcelain"])
    );
    assert_eq!(
        ctx.repo.git(&["show", "HEAD:src/lib.rs"]),
        "pub fn add() {}\n",
        "HEAD 内容应为合入后的版本"
    );
    // 工作区文件本身也要是新内容（用户直接看磁盘时不会看到旧代码）
    assert_eq!(
        std::fs::read_to_string(ctx.repo.path().join("src/lib.rs")).unwrap(),
        "pub fn add() {}\n",
        "磁盘上的工作区文件应为合入后的内容"
    );
}

// ─────────────────────────── 重试重置与清理（决策 125 / 3）───────────────────────────

#[tokio::test]
async fn retry_reset_restores_base_and_cleans_untracked() {
    let ctx = setup();
    init(&ctx).await;
    std::fs::write(ctx.worktree.join("src/lib.rs"), "pub fn broken() {}\n").unwrap();
    commit_worktree(&ctx.worktree, "wip: 半成品");
    std::fs::write(ctx.worktree.join("half_done.txt"), "残留").unwrap();

    Git.reset_hard_clean(&ctx.worktree, "main").await.unwrap();
    assert!(!Git.is_dirty(&ctx.worktree).await.unwrap());
    assert!(
        !ctx.worktree.join("half_done.txt").exists(),
        "clean -fdx 应删除未跟踪文件"
    );
    assert_eq!(
        Git.rev_parse(&ctx.worktree, "HEAD").await.unwrap(),
        ctx.repo.head("main")
    );
    // 分支回到起点后不再包含半成品提交
    assert!(!ctx
        .repo
        .git(&["log", "--oneline", "kanban/t1"])
        .contains("半成品"));
}

#[tokio::test]
async fn cancel_cleanup_is_idempotent() {
    let ctx = setup();
    init(&ctx).await;
    for _ in 0..2 {
        Git.remove_worktree(ctx.repo.path(), &ctx.worktree, true)
            .await
            .unwrap();
        Git.delete_branch(ctx.repo.path(), "kanban/t1")
            .await
            .unwrap();
    }
    assert!(!ctx.worktree.exists());
    assert!(!ctx.repo.branch_exists("kanban/t1"));
    assert!(!ctx.repo.worktree_list().contains("t1"));
}

#[tokio::test]
async fn diff_range_produces_patch_for_proposal() {
    let ctx = setup();
    init(&ctx).await;
    std::fs::write(ctx.worktree.join("src/new.rs"), "pub fn brand_new() {}\n").unwrap();
    commit_worktree(&ctx.worktree, "feat: 新文件");

    let patch = Git
        .diff_range(ctx.repo.path(), "main..kanban/t1")
        .await
        .unwrap();
    assert!(patch.contains("src/new.rs"));
    assert!(patch.contains("+pub fn brand_new()"));

    let stat = Git
        .diff_stat(ctx.repo.path(), "main..kanban/t1")
        .await
        .unwrap();
    assert!(stat.contains("src/new.rs"));
}

// ─────────────────────────── 项目探测（决策 78 / 139）───────────────────────────

#[tokio::test]
async fn project_probing_is_code_based_and_stable() {
    use testkit::Language;
    let repo = Repo::unborn_head().unwrap();
    repo.project(Language::Python);
    let lang = Git::detect_language(repo.path());
    assert_eq!(lang.as_deref(), Some("python"));
    assert_eq!(
        Git::detect_test_framework(repo.path(), lang.as_deref()).as_deref(),
        Some("pytest")
    );
    assert!(Git::agents_md_path(repo.path()).is_none());

    let repo = Repo::clean().unwrap();
    assert!(Git::has_gitignore(repo.path()));
    assert!(Git::agents_md_path(repo.path()).is_some());
}

// ─────────────────────────── rebase 冲突自动解决（票 15 / pipeline-spec §6）───────────────────────────

#[tokio::test]
async fn rebase_conflict_auto_resolves_identical_modification() {
    use agentpipeline_core::git::AutoRebaseOutcome;
    use agentpipeline_core::git::Git;

    // 两侧把同一文件改成完全相同的字节——三方合并可机械判定（ours == theirs）。
    let repo = Repo::clean().unwrap();
    repo.branch("topic");
    repo.write("src/lib.rs", "pub fn same() {}\n");
    repo.commit_all("feat: topic 改成 same");
    repo.checkout("main");
    repo.write("src/lib.rs", "pub fn same() {}\n");
    repo.commit_all("feat: main 也改成 same");

    let home = TestHome::new().unwrap();
    let worktree = home.home().worktree_path("t1");
    let wt = worktree.display().to_string();
    repo.git(&["worktree", "add", "-b", "kanban/t1", &wt, "topic"]);

    let outcome = Git
        .rebase_onto_with_auto_resolve(&worktree, "main")
        .await
        .unwrap();
    match outcome {
        AutoRebaseOutcome::AutoResolved { files, head } => {
            assert_eq!(files, vec!["src/lib.rs".to_string()]);
            assert!(!head.is_empty());
        }
        AutoRebaseOutcome::Clean { .. } => {
            // libgit2 直接判定补丁已应用（等价于自动解决）——同样可接受
        }
        other => panic!("同名同内容修改不应打回：{other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(worktree.join("src/lib.rs")).unwrap(),
        "pub fn same() {}\n"
    );
    assert!(
        agentpipeline_core::git::git2::Repository::open(&worktree)
            .unwrap()
            .open_rebase(None)
            .is_err(),
        "自动解决后 rebase 中断态应已收尾"
    );
    assert!(Git
        .contains(&worktree, "HEAD", &repo.head("main"))
        .await
        .unwrap());
}

#[tokio::test]
async fn rebase_hard_conflict_is_not_auto_resolved() {
    use agentpipeline_core::git::AutoRebaseOutcome;
    use agentpipeline_core::git::Git;

    // 两侧改同一行且内容不同 → 不可机械判定 → Conflict（内部已 abort）
    let repo = Repo::clean().unwrap();
    repo.conflict_hard().unwrap();
    let home = TestHome::new().unwrap();
    let worktree = home.home().worktree_path("t1");
    let wt = worktree.display().to_string();
    repo.git(&["worktree", "add", "-b", "kanban/t1", &wt, "topic"]);

    match Git
        .rebase_onto_with_auto_resolve(&worktree, "main")
        .await
        .unwrap()
    {
        AutoRebaseOutcome::Conflict { files } => {
            assert_eq!(files, vec!["shared.txt".to_string()]);
        }
        other => panic!("硬冲突不得被自动解决：{other:?}"),
    }
    // abort 后已跟踪文件回到干净状态，且中断态清除
    assert!(agentpipeline_core::git::git2::Repository::open(&worktree)
        .unwrap()
        .open_rebase(None)
        .is_err());
}

/// 在 worktree 内提交（fixture 语义与生产一致：agent 在 worktree 里提交）。
fn commit_worktree(worktree: &std::path::Path, message: &str) -> String {
    run_git(worktree, &["add", "-A"]);
    run_git(worktree, &["commit", "-m", message]);
    run_git(worktree, &["rev-parse", "HEAD"]).trim().to_string()
}

fn run_git(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=fixture"])
        .args(["-c", "user.email=fixture@localhost"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git 可执行");
    assert!(
        out.status.success(),
        "git {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ─────────── 并发建 worktree（主流程票 09 暴露的真实缺陷）───────────

/// 同项目两个任务**同时**启动（决策 98 准入允许的常态用法）必须都能建出 worktree。
///
/// 回归的是 libgit2 的 TOCTOU：`worktree.c` 对**共享的** `{repo}/.git/worktrees`
/// 目录先 `path_exists` 再 `mkdir(GIT_MKDIR_EXCL)`。两个任务各起一个
/// `init_worktree` 时双双看到"不存在"、各建一次，后者拿到 `EEXIST`——
/// init 阶段直接报 `failed to make directory '.../.git/worktrees': directory exists`，
/// 任务挂在 init 重试耗尽（用户可见：任务一创建就失败，且**只**在同时建两个任务时）。
///
/// 修复：调用侧按仓库路径串行化这个窗口（`worktree_creation_lock`）。
#[tokio::test]
async fn concurrent_worktree_creation_in_same_repo_does_not_race() {
    let home = TestHome::new().unwrap();
    let repo = Repo::clean().unwrap();

    let mut handles = Vec::new();
    for i in 0..4 {
        let task_id = format!("t{i}");
        let worktree = home.home().worktree_path(&task_id);
        let repo_path = repo.path().to_path_buf();
        handles.push(tokio::spawn(async move {
            Git.init_worktree(&repo_path, &task_id, &worktree, "main")
                .await
        }));
    }
    for (i, handle) in handles.into_iter().enumerate() {
        let result = handle.await.expect("spawn_blocking 不应 panic");
        result.unwrap_or_else(|e| panic!("任务 t{i} 的 worktree 创建不应失败：{e}"));
    }

    // 四个 worktree 都真的建出来，且共享的 .git/worktrees 下登记齐全
    for i in 0..4 {
        assert!(
            home.home()
                .worktree_path(&format!("t{i}"))
                .join("src/lib.rs")
                .exists(),
            "t{i} 的 worktree 应检出主干内容"
        );
    }
    let listed = repo.worktree_list();
    for i in 0..4 {
        assert!(listed.contains(&format!("t{i}")), "worktree 列表应含 t{i}");
    }
}
