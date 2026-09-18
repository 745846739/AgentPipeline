//! 真 GitHub 冒烟（票 01，`#[ignore]` + 显式环境变量，**不进任何自动门**）。
//!
//! 手动运行：
//!
//! ```text
//! AGENTPIPELINE_MARKET_LIVE=1 \
//! cargo test -p agentpipeline-core --test repo_live -- --ignored --nocapture
//! ```
//!
//! **为什么必须两把锁**（`#[ignore]` + 环境变量）：`#[ignore]` 挡住"顺手全跑"，
//! 环境变量挡住"`--ignored` 全跑"——两把都不设就彻底不动网络。测试目录里的其他用例全部
//! 走离线 fixture，默认门一个字节都不过网（决策 194 / 票 01 的验收口径）。
//!
//! **它验的是别处验不到的三件事**（离线 fixture 都是本地造的形态，这三条只有对面的真
//! GitHub 说了算）：
//! 1. 真仓的 `head()` 能取到一个 40 位 SHA（含默认分支的发现：`refs/heads/main`
//!    在广告里叫什么、`HEAD` 指向谁）；
//! 2. 真仓的布局能被**扫描层**认出来（`SKILL.md` 在 2–5 段深度上，而不是本地 fixture
//!    那两三种固定形态）；
//! 3. 按一个**非 tip 的旧 commit** 取一个技能目录能成功（`allow-*-sha1-in-want`
//!    在真 GitHub 的广告里确实是开的——决策 194 的实测底稿就是这么来的）。
//!
//! **已知边界**：真网络会偶发中断（历史上 75 s 超时一次）。它是**人工确认手段**，
//! 不是回归门——绿不保证明天绿；价值是把"真 GitHub 路径完全无人知晓"变成"发版前有人跑过"。

use std::path::PathBuf;

use agentpipeline_core::agent::repo::{Libgit2Repo, Oid, RepoId, SkillRepo};

/// 冒烟用的真仓：一个**多技能、深度 2 段**的公开仓（实测 8 个技能）。
const REPO: &str = "mattpocock/skills";

fn live_enabled() -> bool {
    std::env::var("AGENTPIPELINE_MARKET_LIVE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// 每次一个独立缓存目录（与契约用例同一条理由：共用一个会让"取过了"混进读数）。
fn cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentpipeline-live-repo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore = "打真 GitHub；用法见文件头注释"]
async fn real_github_head_scan_and_old_commit() {
    if !live_enabled() {
        eprintln!("未设置 AGENTPIPELINE_MARKET_LIVE=1，跳过真 GitHub 冒烟");
        return;
    }
    let repo = RepoId::parse(REPO).unwrap();
    let client = Libgit2Repo::new(cache_dir());

    // ① 取 tip：40 位十六进制，且能与系统 git 对齐（这里只断言形态——与 `ls-remote` 对表
    //    的价值不大，形态错了后面两步必然失败）。
    let tip = client.head(&repo).await.expect("取 tip 失败");
    assert_eq!(tip.as_str().len(), 40, "tip 应当是完整 SHA：{tip}");
    println!("tip = {}", tip.as_str());

    // ② 扫描层：真仓是多技能仓，技能目录深度 2–5 段（决策 194 实测 290/290 成立）
    let skills = client.list_skills(&repo, &tip).await.expect("列技能失败");
    assert!(!skills.is_empty(), "这个仓不该是空的");
    println!("扫到 {} 个技能", skills.len());
    for s in skills.iter().take(5) {
        println!("  {} → {}", s.name, s.dir);
    }
    // 名字 = 含 SKILL.md 那个目录的 basename，且 dir 里必有它自己
    for s in &skills {
        assert_eq!(
            s.dir.rsplit('/').next().unwrap(),
            s.name,
            "名字应当是目录 basename：{s:?}"
        );
    }

    // ③ 按裸 SHA 取一个**非 tip** 的 commit，验证真 GitHub 上 `allow-*-sha1-in-want`
    //    确实是开的——离线 fixture 开那个位就是为了与它一致（决策 194 的实测底稿）。
    let older = an_older_ref().await;
    assert_ne!(older.as_str(), tip.as_str(), "要挑一个不是 tip 的 commit");
    let listed = client
        .list_skills(&repo, &older)
        .await
        .expect("按旧 commit 列技能失败");
    println!("旧 commit {} 上扫到 {} 个技能", older.short(), listed.len());

    // ④ 读一个技能目录：拿到的是重打成 `{name}/SKILL.md` 的包
    let target = skills
        .iter()
        .find(|s| s.dir.split('/').count() >= 3)
        .expect("这个仓里应当有多段深度的技能");
    let package = client
        .read_skill(&repo, &tip, &target.dir)
        .await
        .expect("读技能目录失败");
    assert_eq!(package.name, target.name);
    assert!(
        package
            .files
            .contains_key(&format!("{}/SKILL.md", target.name)),
        "包内应当有 {}/SKILL.md：{:?}",
        target.name,
        package.files.keys().collect::<Vec<_>>()
    );
    println!(
        "读到 {}/SKILL.md（{} 个文件）",
        target.name,
        package.files.len()
    );
}

/// 挑一个**不是 tip** 的已知 commit：用系统 git 问一次 `ls-remote`（不把"谁是旧提交"
/// 这件事交给被测代码去算），取一个 tag 指向的提交。
///
/// **要取剥出来的那一行**（`refs/tags/x^{}`）：注解标签在 `ls-remote` 里出现两次——一次是
/// **标签对象**自己的 SHA，一次是它指向的 commit。取错了那一行，被测代码拿到的是一个
/// 合法但**不是 commit** 的对象，`find_commit` 会以
/// `the requested type does not match the type in the ODB` 失败（本用例第一次跑就踩到了，
/// 分类落成 `commit_not_found` 是对的——那个 SHA 确实不是一个能装的 commit）。
async fn an_older_ref() -> Oid {
    let out = std::process::Command::new("git")
        .args(["ls-remote", &format!("https://github.com/{REPO}.git")])
        .output()
        .expect("git 可执行");
    assert!(out.status.success(), "ls-remote 失败");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let line = stdout
        .lines()
        .find(|l| l.contains("refs/tags/") && l.contains("^{}"))
        .or_else(|| stdout.lines().last())
        .unwrap_or_else(|| panic!("ls-remote 没给出任何 ref：{stdout}"));
    let sha = line.split_whitespace().next().expect("ref 行应有 SHA");
    Oid::parse(sha).unwrap_or_else(|e| panic!("ref 行的 SHA 不合法（{sha}）：{e}"))
}
