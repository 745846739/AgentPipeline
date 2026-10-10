//! 票 `gate-frontend-tests` 01（选型 C）的**配置面形状钉**：前端单测进流水线闸门，
//! 落点是 Makefile `check-test` 扩面（决策 168：本文件是闸门的唯一权威定义）与
//! 项目注册值 `test_framework = make check-test`。闸门跑的是**任务 worktree** 里
//! 的这份 Makefile——worktree 没有 node_modules，前端步必须自带依赖兜底，
//! 否则闸门把所有任务拦死在 develop。负例（测试命令失败 → validate_output 红）
//! 的流水线侧牙齿在 `crates/core/tests/integration/executor.rs::a_failing_test_command_*`。

use std::path::PathBuf;

fn root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // tests/e2e
    p.pop(); // tests
    p
}

/// `check-test` = cargo 全量 + 前端单测，且前端步在 node_modules 缺失/过期时先
/// `npm ci`（任务 worktree 与干净 runner 都靠这一步才有 vitest 可跑）。
#[test]
fn check_test_target_carries_the_frontend_suite_with_a_dependency_fallback() {
    let makefile = std::fs::read_to_string(root().join("Makefile")).expect("读 Makefile");
    let start = makefile
        .find("\ncheck-test:")
        .expect("check-test 目标应存在");
    let rest = &makefile[start..];
    let end = rest
        .find("\ncheck-frontend:")
        .expect("check-frontend 应仍是后一个目标");
    let body = &rest[..end];
    assert!(
        body.contains("cargo test --workspace"),
        "check-test 仍要跑 cargo 全量：{body}"
    );
    assert!(
        body.contains("npm test"),
        "check-test 必须带上前端单测（本票的扩面本体）：{body}"
    );
    assert!(
        body.contains("node_modules") && body.contains("npm ci"),
        "前端步必须带依赖兜底（任务 worktree 没有 node_modules）：{body}"
    );
}

/// CI 的 test job 要给 `make check-test` 备好 node 环境——runner 每次是干净的，
/// 没有 setup-node + 装依赖，扩面后的 check-test 必红。
#[test]
fn ci_test_job_provisions_node_for_the_widened_check_test() {
    let yml =
        std::fs::read_to_string(root().join(".github/workflows/check.yml")).expect("读 check.yml");
    // 锚在**结构行**（`  test:`）而不是 job 显示名——标题改字不应使形状钉失效；
    // 边界也硬性 expect（frontend job 不在了就该红，不许静默把扫描扩到文件尾）。
    let test_job = yml
        .find("\n  test:")
        .map(|i| &yml[i..])
        .expect("test job 应存在");
    let next_job = test_job
        .find("\n  frontend:")
        .expect("frontend job 应仍是后一个 job");
    let job = &test_job[..next_job];
    assert!(
        job.contains("make check-test"),
        "test job 跑的应是扩面后的 check-test：{job}"
    );
    assert!(
        job.contains("actions/setup-node@v4"),
        "test job 需要 node（扩面后的 check-test 要跑 vitest）：{job}"
    );
    assert!(
        job.contains("npm ci"),
        "runner 每次干净，依赖必须无条件装（口径照 frontend job）：{job}"
    );
}
