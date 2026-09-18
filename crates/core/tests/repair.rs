//! L2 集成：修复的载体与交付（决策 210③④⑤ / 票 10、11）。
//!
//! 测的是**真 git**：worktree 真的拉起来了、分支真的从 base 分出、commit 真的在、
//! diff 的范围真的是 `{base}..{branch}`、回收之后目录真的没了而分支按规则留着。
//! 不 mock git：这一票的全部风险都在「git 的那几步做对了没有」上。

use std::sync::Arc;

use agentpipeline_core::agent::file_policy::foreman_file_policy;
use agentpipeline_core::git::Git;
use agentpipeline_core::pipeline::repair::{
    commit_repair, finish_repair, gate_failure_note, new_repair_id, repair_diff, repair_head,
    run_repair_gate, start_repair, write_repair_diff, REPAIR_COMMIT_MARK,
};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::Project;
use testkit::{ManualClock, Repo, TestHome};

async fn fixture() -> (TestHome, Store, ManualClock, Repo) {
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
        .await
        .unwrap();
    let repo = Repo::clean().unwrap();
    (home, store, clock, repo)
}

/// 票 10：从零拉起一个修复 worktree——分支从正确 base 分出、目录在家下、**值班长写得进**。
#[tokio::test]
async fn a_repair_worktree_lives_under_home_and_is_writable_by_the_foreman() {
    let (home, _store, _clock, repo) = fixture().await;
    let base_before = repo.head("main");
    let repair_id = new_repair_id();
    let session = start_repair(home.home(), repo.path(), "main", &repair_id)
        .await
        .unwrap();

    assert!(session.branch.starts_with("repair/"), "修复分支要有自己的前缀");
    assert_eq!(session.base_ref, "main", "无 remote 时基准是本地分支");
    assert_eq!(session.base_ref, "main");
    assert_eq!(
        repo.head(&session.branch),
        base_before,
        "修复分支应当从基准的同一个 commit 分出"
    );
    // 目录在**家目录下**（值班长的写域之内）——这不是审美，是可达性的硬约束
    let root = home.home().root();
    assert!(
        session.worktree.starts_with(root),
        "worktree 必须落在 {} 之下：{}",
        root.display(),
        session.worktree.display()
    );
    assert!(session.worktree.exists());
    // 文件策略：值班长写得进它（**有断言钉住**，票面明写要求）
    let policy = foreman_file_policy(root);
    let target = session.worktree.join("crates/core/src/lib.rs");
    assert!(
        policy.check_write(&target).is_ok(),
        "修复 worktree 必须落在值班长的写域内：{}",
        target.display()
    );
    // 反面：本仓自己的工作区**写不进**（这正是要 worktree 的原因）
    assert!(
        policy.check_write(&repo.path().join("src/lib.rs")).is_err(),
        "值班长不该能写项目工作区——那条路靠的是 worktree"
    );
}

/// 票 10：两个修复并发拉同一仓库的 worktree，**都成功**（`with_worktree_lock` 的牙齿）。
#[tokio::test]
async fn concurrent_repair_worktrees_for_the_same_repo_all_succeed() {
    let (home, _store, _clock, repo) = fixture().await;
    let path = repo.path().to_path_buf();
    let root = home.home().clone();
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let root = root.clone();
        handles.push(tokio::spawn(async move {
            let id = new_repair_id();
            start_repair(&root, &path, "main", &id).await
        }));
    }
    for handle in handles {
        let session = handle.await.unwrap().expect("并发建 worktree 不该撞 EEXIST");
        assert!(session.worktree.exists());
    }
}

/// 票 10 的回收规则：**合入成功**删分支 + 删 worktree；**被拒**保留分支、删 worktree。
#[tokio::test]
async fn cleanup_keeps_the_branch_unless_it_was_merged() {
    let (home, _store, _clock, repo) = fixture().await;
    // 被拒 / 年龄清理那一路
    let rejected = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    rejected_repair(&rejected, repo.path()).await;
    let merged = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    finish_repair(repo.path(), &merged, true).await.unwrap();

    // 一起断言（两次都在同一个仓上）
    async fn rejected_repair(
        session: &agentpipeline_core::pipeline::repair::RepairSession,
        repo: &std::path::Path,
    ) {
        finish_repair(repo, session, false).await.unwrap();
        assert!(!session.worktree.exists(), "worktree 应当被删掉");
        assert!(
            Git.rev_parse(repo, &session.branch).await.is_ok(),
            "被拒的修复要保留分支——它是唯一的证据"
        );
    }
    assert!(!merged.worktree.exists());
    assert!(
        Git.rev_parse(repo.path(), &merged.branch).await.is_err(),
        "合入成功的修复分支应当被删掉"
    );
}

/// 票 11：闸门不过 → **不出 diff**，且失败说明点名是哪一步（lint 还是 test）。
#[tokio::test]
async fn a_failed_gate_produces_no_diff() {
    let (home, store, _clock, repo) = fixture().await;
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    // 在修复 worktree 里改点东西，再让闸门必然失败
    std::fs::write(session.worktree.join("add.rs"), "pub fn x() {}\n").unwrap();
    let readings = run_repair_gate(
        &store,
        home.home(),
        &session,
        None,
        Some("false"), // 测试命令恒非零（fixture 惯例）
    )
    .await
    .unwrap();
    assert_eq!(readings.len(), 1, "lint 未配置时只跑测试");
    assert_ne!(readings[0].exit_code, 0);
    let note = gate_failure_note(&readings);
    assert!(note.contains("test"), "{note}");
    assert!(
        note.contains("gate-output-repair-"),
        "读数要指向落盘的全文：{note}"
    );
    // 闸门不过 → 调用方不该走到 commit / diff（这里直接断言「没有 commit」）
    assert!(
        Git.rev_parse(repo.path(), &session.branch).await.is_ok(),
        "分支仍在（改动没被丢掉）"
    );
    let branch_head = repo.head(&session.branch);
    let base_head = repo.head("main");
    assert_eq!(branch_head, base_head, "没有 commit：两者还在同一个 commit 上");
}

/// 票 11：闸门过了 → commit 存在且 message 带标记；diff 的范围是 `{base}..{branch}`。
#[tokio::test]
async fn a_passing_gate_leads_to_a_marked_commit_and_a_scoped_diff() {
    let (home, store, clock, repo) = fixture().await;
    let before = repo.head("main");
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    std::fs::write(
        session.worktree.join("fixed.rs"),
        "pub fn fixed() -> i32 { 42 }\n",
    )
    .unwrap();

    let readings = run_repair_gate(&store, home.home(), &session, None, Some("true"))
        .await
        .unwrap();
    assert_eq!(readings[0].exit_code, 0, "闸门通过");
    let commit = commit_repair(
        &session,
        "develop 阶段缺了一条约束",
        agentpipeline_core::clock::Clock::now(&clock),
    )
        .await
        .unwrap();
    let (diff, stat) = repair_diff(repo.path(), &session).await.unwrap();

    assert_eq!(repair_head(repo.path(), &session).await.unwrap(), commit);
    let message = repo.git(&["log", "--format=%s", "-1", &session.branch]);
    assert!(
        message.contains(REPAIR_COMMIT_MARK),
        "commit message 要带可检索的标记：{message}"
    );
    assert!(
        message.contains("develop 阶段缺了一条约束"),
        "诊断结论那一句要跟着进 message：{message}"
    );
    assert!(diff.contains("fixed.rs"), "diff 要带上改动：{diff}");
    assert!(stat.contains("fixed.rs"), "diff stat 同上：{stat}");
    // 范围就是 `{base}..{branch}`：base 那一侧的 commit 不出现在 diff 里
    assert_eq!(session.base_ref, "main");
    let range = format!("{}..{}", session.base_ref, session.branch);
    assert_eq!(
        repo.git(&["rev-list", "--count", &range]).trim(),
        "1",
        "只有一个修复 commit"
    );
    assert_ne!(before, commit);
    // diff 可落盘给人看（提议面板要能展开）
    let path = write_repair_diff(home.home(), &session.repair_id, &diff).unwrap();
    assert!(path.exists());
}

/// 票 11：本仓那一路**不许热修、不许自己重启**——修复产物一律落在 worktree 里。
#[tokio::test]
async fn the_repair_never_touches_the_project_working_tree() {
    let (home, store, clock, repo) = fixture().await;
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    std::fs::write(session.worktree.join("x.rs"), "pub fn x() {}\n").unwrap();
    let _ = run_repair_gate(&store, home.home(), &session, None, Some("true")).await;
    let _ = commit_repair(
        &session,
        "结论",
        agentpipeline_core::clock::Clock::now(&clock),
    )
    .await;

    assert!(
        !repo.is_dirty(),
        "项目工作区必须干净：修复只碰 worktree（本仓不许热修）"
    );
    assert!(
        !repo.exists("x.rs"),
        "改动不该出现在项目工作区里"
    );
}

/// 修复的判据：不是 git 仓就说清「为什么不能修」。
#[tokio::test]
async fn a_non_git_project_says_why_it_cannot_be_repaired() {
    let (home, _store, _clock, _repo) = fixture().await;
    let plain = home.scratch_dir("plain");
    let project = Project {
        id: "p-plain".into(),
        name: "裸目录".into(),
        local_path: plain.display().to_string(),
        default_branch: "main".into(),
        language: None,
        test_framework: None,
        lint_command: None,
        agents_md_path: None,
        created_at: chrono::Utc::now(),
    };
    let err = agentpipeline_core::pipeline::repair::repair_supported(&project).unwrap_err();
    assert!(err.to_string().contains("不是 git 仓库"), "{err}");
}

// ─────────────── 修复提议：不设 TTL + 指纹换义（决策 212① / 票 12）───────────────

/// 修复提议**不按时间过期**：睡过一夜后仍是 pending、按钮仍可点。
#[tokio::test]
async fn a_repair_proposal_survives_the_night() {
    use agentpipeline_core::pipeline::repair::propose_repair;
    use agentpipeline_core::storage::proposals::ForemanProposalKind;

    let (home, store, clock, repo) = fixture().await;
    let project = Project {
        id: "p1".into(),
        name: "示例".into(),
        local_path: repo.path().display().to_string(),
        default_branch: "main".into(),
        language: None,
        test_framework: Some("true".into()),
        lint_command: None,
        agents_md_path: None,
        created_at: agentpipeline_core::clock::Clock::now(&clock),
    };
    let session_id = store.create_foreman_session("夜班").await.unwrap().id;
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    std::fs::write(session.worktree.join("x.rs"), "pub fn x() {}\n").unwrap();
    let gate = run_repair_gate(&store, home.home(), &session, None, Some("true"))
        .await
        .unwrap();
    let commit = commit_repair(
        &session,
        "结论一句话",
        agentpipeline_core::clock::Clock::now(&clock),
    )
    .await
    .unwrap();
    let (diff, stat) = repair_diff(repo.path(), &session).await.unwrap();
    let outcome = agentpipeline_core::pipeline::repair::RepairOutcome {
        repair_id: session.repair_id.clone(),
        worktree_path: session.worktree.display().to_string(),
        branch: session.branch.clone(),
        base_ref: session.base_ref.clone(),
        base_commit: repo.head("main"),
        gate_passed: true,
        gate: gate.clone(),
        commit: Some(commit),
        diff: Some(diff),
        diff_stat: Some(stat),
    };
    let proposal = propose_repair(&store, &session_id, &project, &outcome)
        .await
        .unwrap();
    assert_eq!(proposal.kind, ForemanProposalKind::Repair);
    assert!(proposal.payload.is_some(), "载荷里要有 diff 与闸门读数");

    // 八小时之后（一夜）仍在有效期内，且状态仍是 pending
    let morning = agentpipeline_core::clock::Clock::now(&clock) + chrono::Duration::hours(8);
    assert!(!proposal.is_expired(morning), "修复提议不该按 10 分钟过期");
    assert!(proposal.status.is_open(), "早上看到的应当是可按键的");
    // 对照：普通提议在同样的时刻早已过期（10 分钟 TTL）
    let api_call = store
        .create_foreman_proposal(agentpipeline_core::storage::proposals::NewForemanProposal {
            session_id: session_id.clone(),
            tool: "run_command".into(),
            args: serde_json::json!({"command": "ls"}),
            summary: "执行命令：ls".into(),
            situation: None,
            kind: ForemanProposalKind::ApiCall,
            payload: None,
        })
        .await
        .unwrap();
    assert!(api_call.is_expired(morning), "普通提议照旧 10 分钟过期");
}

/// 指纹换义（票 12）：基准前进了 —— 执行时能干净 rebase 就合，冲突就报出文件清单。
#[tokio::test]
async fn the_repair_rebase_check_speaks_up_on_conflicts() {
    let (home, store, clock, repo) = fixture().await;
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    // 修复分支改一行
    std::fs::write(session.worktree.join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a - b }\n").unwrap();
    let _ = run_repair_gate(&store, home.home(), &session, None, Some("true")).await;
    let _ = commit_repair(
        &session,
        "改掉一个符号",
        agentpipeline_core::clock::Clock::now(&clock),
    )
    .await;

    // 基准前进了**没有冲突**的一步：干净 rebase
    repo.advance_main("README.md", "# 新的一行\n");
    let outcome = Git
        .rebase_onto_with_auto_resolve(&session.worktree, "main")
        .await
        .unwrap();
    assert!(
        !matches!(
            outcome,
            agentpipeline_core::git::AutoRebaseOutcome::Conflict { .. }
        ),
        "不冲突时应当能合：{outcome:?}"
    );

    // 基准又前进，且改的是**同一段**：冲突，且文件清单说得出来
    let session2 = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    std::fs::write(
        session2.worktree.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a * b }\n",
    )
    .unwrap();
    let _ = commit_repair(
        &session2,
        "另一种改法",
        agentpipeline_core::clock::Clock::now(&clock),
    )
    .await;
    repo.advance_main("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { b + a }\n");
    let outcome = Git
        .rebase_onto_with_auto_resolve(&session2.worktree, "main")
        .await
        .unwrap();
    match outcome {
        agentpipeline_core::git::AutoRebaseOutcome::Conflict { files } => {
            assert!(
                files.iter().any(|f| f.contains("lib.rs")),
                "冲突文件要说得出：{files:?}"
            );
        }
        other => panic!("同一段被两边改过应当冲突：{other:?}"),
    }
}

/// 合入成功那一路：分支合进默认分支 + 回收（删分支、删 worktree）。
#[tokio::test]
async fn merging_a_repair_lands_the_branch_and_cleans_up() {
    let (home, _store, clock, repo) = fixture().await;
    let session = start_repair(home.home(), repo.path(), "main", &new_repair_id())
        .await
        .unwrap();
    std::fs::write(session.worktree.join("added.rs"), "pub fn y() {}\n").unwrap();
    let _ = commit_repair(
        &session,
        "补一个文件",
        agentpipeline_core::clock::Clock::now(&clock),
    )
    .await;
    // 无 remote 时基准是本地 main：先把 main 的引用对齐（fixture 的 main 就是 HEAD）
    Git.merge_into_default_branch(repo.path(), "main", &session.branch)
        .await
        .unwrap();
    finish_repair(repo.path(), &session, true).await.unwrap();

    assert!(repo.exists("added.rs"), "改动应当出现在主干上");
    assert!(!session.worktree.exists(), "worktree 被回收");
    assert!(
        Git.rev_parse(repo.path(), &session.branch).await.is_err(),
        "合入成功后分支被删"
    );
}
