//! 命令收口（决策 297 / 票 01–04）：进程树、进程组回填、改写与 shim 前置。
//!
//! 这一份测的是 [`agentpipeline_core::exec::CommandRunner`] **本身**的性质——它是四条命令
//! 写路径（agent `run_command` / `run_readonly` / 执行器闸门 / 修复闸门）共用的那条管道。
//! 闸门**确实走这条管道**由 `repair` 那一份钉住（台账归属、脱敏、无 `original_command`），
//! 这里钉住的是管道本身的几条硬性质。
//!
//! 为什么值得单独一份：收口之前的两个闸门是 `sh -c` 旁路，超时**只丢 future、不杀进程**
//! （退出后子孙还在跑），也从不回填 `process_group_id`（调度器那条超时收口够不着它）。
//! 这两条都是「没有测试所以没人发现」的形状。

use std::sync::Arc;

use agentpipeline_core::agent::tools::CommandFinish;
use agentpipeline_core::exec::{CommandOwner, CommandRequest, CommandRunner, Rewrite, SpawnForm};
use agentpipeline_core::process::RealProcessKiller;
use agentpipeline_core::rtk::RtkRuntime;
use agentpipeline_core::storage::observability::NewRun;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{CommandSource, Node, Stage};
use testkit::{ManualClock, TestHome};

struct Fixture {
    _home: TestHome,
    store: Store,
    task_id: String,
    run_id: i64,
    cwd: std::path::PathBuf,
}

async fn fixture() -> Fixture {
    let home = TestHome::new().unwrap();
    let store = Store::open(home.home().clone(), Arc::new(ManualClock::fixed()))
        .await
        .unwrap();
    let repo = home.scratch_dir("proj");
    testkit::seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    testkit::seed_task(&store, "t1", "p1").await.unwrap();
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    let cwd = home.home().worktree_path("t1");
    std::fs::create_dir_all(&cwd).unwrap();
    let run_id = store
        .insert_run(&NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id,
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    Fixture {
        _home: home,
        store,
        task_id: "t1".into(),
        run_id,
        cwd,
    }
}

impl Fixture {
    /// 一条**闸门形状**的请求：`source = System`、不改写。
    fn request<'a>(&'a self, command: &'a str, timeout_sec: u64) -> CommandRequest<'a> {
        CommandRequest {
            owner: CommandOwner {
                task_id: Some(self.task_id.clone()),
                session_id: None,
                run_id: Some(self.run_id),
                stage: Stage::Develop,
                node: Node::Execute,
                source: CommandSource::System,
            },
            command,
            cwd: &self.cwd,
            timeout_sec,
            spawn: SpawnForm::Shell,
            rewrite: Rewrite::None,
        }
    }

    fn runner(&self) -> CommandRunner {
        CommandRunner::new(Arc::new(RealProcessKiller)).with_recorder(Arc::new(self.store.clone()))
    }
}

/// 一条命令连同它的读数的收尾（本文几乎所有用例都走这一条）。
fn trivial(
    out: &agentpipeline_core::exec::CommandOutput,
) -> agentpipeline_core::Result<(CommandFinish, ())> {
    Ok((
        CommandFinish {
            exit_code: out.exit_code,
            duration_ms: out.duration_ms,
            ..Default::default()
        },
        (),
    ))
}

fn pid_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 改写是 **fail-open** 的（决策 297 / 票 03）：2s 内没回来就当没这回事，命令原样跑。
///
/// 全套测试并行跑时机器满载，一次尝试偶发超时是**这条设计允许的结果**，不是回归——
/// 而它确实会撞上（本机复现过：满载下单跑这一条，10 次里 1 次踩到 2s）。故凡是要断言
/// 「改写真的发生了」的用例都走这个有界重试：**试到看见改写为止**，并把每一次都过一遍
/// 台账两列的自洽（要么两份都在、要么都没有——半个改写不是一种状态）。
///
/// 试满还不改写才算失败：那说明开关没通到收口，或者 shim 根本解析不到 rtk。
const REWRITE_ATTEMPTS: usize = 5;

fn ledger_is_consistent(row: &agentpipeline_core::types::NodeCommand) {
    match &row.original_command {
        Some(original) => {
            assert_ne!(row.command, *original, "改写过的行，两列不该一样");
        }
        None => assert!(!row.command.is_empty(), "没改写时记的就是原串，不该是空的"),
    }
}

/// 超时**真的把进程树收掉**（票 02 的判据：收口之前的 runner 闸门过不了这条）。
///
/// 命令故意留一个脱离管道的子孙（`sleep 300 &`）：只杀掉直接子进程的实现会让它活下来
/// ——那正是「超时丢 future」的后果。杀的是**进程组**，故子孙一起走。
#[tokio::test]
async fn a_timed_out_command_takes_its_descendants_with_it() {
    let fx = fixture().await;
    let pid_file = fx.cwd.join("descendant.pid");
    let command = format!("sleep 300 & echo $! > {}; sleep 300", pid_file.display());

    let (out, ()) = fx
        .runner()
        .run(fx.request(&command, 1), trivial)
        .await
        .unwrap();
    assert!(out.timed_out, "1s 上限下 `sleep 300` 必须判超时");
    assert_eq!(out.exit_code, None, "超时的命令没有退出码");

    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("子孙应当写下自己的 pid")
        .trim()
        .parse()
        .unwrap();
    // TERM 到进程组之后，子孙是立刻就没了还是要给它一小段时间——轮询而不是硬等。
    let mut gone = false;
    for _ in 0..50 {
        if !pid_alive(pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        gone,
        "超时后子孙进程 {pid} 还活着——超时只丢了 future，没杀进程组"
    );
}

/// 闸门形状的命令也要**回填 `process_group_id`**（票 02 的判据）。
///
/// 这一列是调度器那条超时收口唯一的抓手（决策 66 / 票 17）：不填，`kill(-pgid)` 就是
/// no-op；收口之前的两个闸门正是从不填它。
#[tokio::test]
async fn a_source_system_command_backfills_its_process_group() {
    let fx = fixture().await;
    let (out, ()) = fx
        .runner()
        .run(fx.request("echo hi", 30), trivial)
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout.starts_with("hi"), "{:?}", out.stdout);

    let run = fx
        .store
        .list_runs(&fx.task_id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == fx.run_id)
        .unwrap();
    assert!(
        run.process_group_id.unwrap_or(0) > 0,
        "闸门命令也要回填真实 pgid：{:?}",
        run.process_group_id
    );
}

/// 闸门的台账行**没有 `original_command`**（票 02 / 票 03 的判据）。
///
/// 「闸门不改写」这件事在台账上的样子就是这一格恒为 NULL——与「开关关着」「`run_readonly`」
/// 同一件事（三种「按原样跑」对台账是同一种）。
#[tokio::test]
async fn a_source_system_command_records_no_original_command() {
    let fx = fixture().await;
    let (out, ()) = fx
        .runner()
        .run(fx.request("echo hi", 30), trivial)
        .await
        .unwrap();
    let row = fx
        .store
        .get_command(out.command_id.expect("落过台账"))
        .await
        .unwrap()
        .expect("那一行在");
    assert_eq!(row.original_command, None, "闸门不记原串：它从不改写");
    assert_eq!(row.source.as_str(), "system");
    assert_eq!(row.command, "echo hi", "执行的就是原文（脱敏之后）");
}

/// 假 rtk：既当**改写器**（`rtk hook claude`），又当被改写出来的命令（`rtk …`）。
///
/// 两个角色由 `$1` 分开——这正是运行期真实的形状：改写器吐裸 `rtk`，那条命令靠 shim
/// 解析回**同一个二进制**。
///
/// 只在 payload 里认得出 `trigger` 时才回改写（真 rtk 也是这个形状：名单外的命令回空输出、
/// 等于不改写）。于是「哪些命令被改写」在测试里是可控的，别地把「没被改写」当噪声。
fn fake_rtk(dir: &std::path::Path, trigger: &str, rewritten: &str) -> std::path::PathBuf {
    let path = dir.join("rtk");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"hook\" ]; then\n\
               read -r payload\n\
               case \"$payload\" in\n\
                 *{trigger}*) printf '%s' '{{\"hookSpecificOutput\":{{\"updatedInput\":{{\"command\":\"{rewritten}\"}}}}}}' ;;\n\
                 *) : ;;\n\
               esac\n\
             else\n\
               echo SHIM-RAN \"$@\"\n\
             fi\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// 钉住的 shim 目录 + 预置的假 rtk（票 04 的运行期形状）。
fn pinned(
    home: &testkit::TestHome,
    trigger: &str,
    rewritten: &str,
) -> (Arc<RtkRuntime>, std::path::PathBuf) {
    let bin_dir = home.scratch_dir("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let binary = fake_rtk(&bin_dir, trigger, rewritten);
    let shim_dir = home.home().root().join("rtk-shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    // 用生产那条 `pin` 而不是自己造链接：钉的动作与被钉的东西要一起被测。
    agentpipeline_core::rtk::pin(home.home(), &binary).unwrap();
    (Arc::new(RtkRuntime { binary, shim_dir }), bin_dir)
}

/// 票 04 的存在理由：**子进程靠 shim 找到 rtk**，而不是靠服务进程的 `PATH`。
///
/// 断言的是「跑起来的是 shim 里那一份」——用一个会自报家门的假 rtk 钉住。这条对环境
/// 无关：shim 前置在最前，故即便这台机器的 `PATH` 里另有一个真 rtk，赢的也是 shim。
#[tokio::test]
async fn the_shim_is_what_resolves_rtk_for_the_child() {
    let fx = fixture().await;
    let (runtime, _bin_dir) = pinned(&fx._home, "ls", "rtk marker");

    // ① 有 shim：改写器把 `ls` 换成 `rtk marker`，那条命令靠 PATH 解析回**同一个**假 rtk
    let command = "ls";
    let mut rewritten = None;
    for _ in 0..REWRITE_ATTEMPTS {
        let (out, row) = run_pinned(&fx, &runtime, command).await;
        ledger_is_consistent(&row);
        if row.original_command.is_some() {
            assert_eq!(out.exit_code, Some(0));
            assert!(
                out.stdout.starts_with("SHIM-RAN marker"),
                "被改写出来的裸 `rtk` 必须靠 shim 解析到钉住的那一份：{:?}",
                out.stdout
            );
            rewritten = Some(row);
            break;
        }
    }
    let row = rewritten.unwrap_or_else(|| {
        panic!("连试 {REWRITE_ATTEMPTS} 次都没有改写：改写器挂住了，或 shim 解析不到 rtk")
    });
    assert_eq!(row.command, "rtk marker", "台账记的是**实际执行的**那条");
    assert_eq!(
        row.original_command.as_deref(),
        Some(command),
        "原串也要在：`cat` 与 `rtk read` 的输出不一样，只记一份排障会看错"
    );

    // ② 没有 shim（开关关着）：原样跑 `ls`，假 rtk 完全没被碰
    let (out, ()) = fx
        .runner()
        .run(fx.request(command, 30), trivial)
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(
        !out.stdout.contains("SHIM-RAN"),
        "没有 shim 时不该有那条命令：{:?}",
        out.stdout
    );
}

/// 用**钉住的那一份** rtk 跑一条命令（绕过开关，直接给收口一个运行期读数）。
async fn run_pinned(
    fx: &Fixture,
    runtime: &Arc<RtkRuntime>,
    command: &str,
) -> (
    agentpipeline_core::exec::CommandOutput,
    agentpipeline_core::types::NodeCommand,
) {
    let (out, ()) = CommandRunner::new(Arc::new(RealProcessKiller))
        .with_recorder(Arc::new(fx.store.clone()))
        .with_rtk(Some(runtime.clone()))
        .run(
            CommandRequest {
                rewrite: Rewrite::Rtk,
                ..fx.request(command, 30)
            },
            trivial,
        )
        .await
        .unwrap();
    let row = fx
        .store
        .get_command(out.command_id.expect("落过台账"))
        .await
        .unwrap()
        .expect("那一行在");
    (out, row)
}

/// shim 目录里只有 `rtk` 一个名字，故前置它**不改别的命令的解析**（票 04 的判据）。
///
/// 这是它与「把 `/usr/local/bin` 前置」的分界线：那个目录里还有一堆别的二进制，
/// 前置它会顺手改掉 `python3` 之类别的命令的解析。
#[tokio::test]
async fn prepending_the_shim_does_not_change_other_resolutions() {
    let fx = fixture().await;
    let (runtime, _bin_dir) = pinned(&fx._home, "ls", "rtk marker");
    let probe = "command -v python3; command -v sh";

    // 不带 shim 的读数（基线）
    let (baseline, ()) = fx
        .runner()
        .run(fx.request(probe, 30), trivial)
        .await
        .unwrap();
    let (with_shim, ()) = CommandRunner::new(Arc::new(RealProcessKiller))
        .with_recorder(Arc::new(fx.store.clone()))
        .with_rtk(Some(runtime))
        .run(
            CommandRequest {
                rewrite: Rewrite::Rtk,
                ..fx.request(probe, 30)
            },
            trivial,
        )
        .await
        .unwrap();
    assert_eq!(
        with_shim.stdout, baseline.stdout,
        "shim 只该影响 `rtk` 这一个名字：别的命令解析到哪还是哪"
    );
}

/// 改写只作用于**走 shell 的命令**：argv 直出的 `run_readonly` 永不改写（决策 232 的安全面）。
///
/// 这一条在调用点（`run_readonly` 传 `Rewrite::None`）与收口内部（`SpawnForm::Argv` 直接
/// 返回）各挡一次——这里钉的是后者，免得将来有人只看调用点。
#[tokio::test]
async fn an_argv_command_is_never_rewritten() {
    let fx = fixture().await;
    let (runtime, _bin_dir) = pinned(&fx._home, "ls", "rtk marker");
    let args = vec!["hi".to_string()];
    let (out, ()) = CommandRunner::new(Arc::new(RealProcessKiller))
        .with_recorder(Arc::new(fx.store.clone()))
        .with_rtk(Some(runtime))
        .run(
            CommandRequest {
                spawn: SpawnForm::Argv {
                    program: "echo",
                    args: &args,
                },
                rewrite: Rewrite::Rtk,
                ..fx.request("echo hi", 30)
            },
            trivial,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout.starts_with("hi"), "{:?}", out.stdout);
    let row = fx
        .store
        .get_command(out.command_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.original_command, None, "argv 直出那一支永不改写");
}

// ─────────────────── 生产接线：agent 那条路（`ToolExecutor` → 收口）───────────────────

/// 开关**打开**时 `run_command` 真的走 rtk；关着时逐字等于今天（票 03 的判据）。
///
/// 这一条钉的是**接线**：`ToolExecutor` 必须把库里那一行接给收口（`with_rtk_store`）——
/// 少了它，改写全是空转，而单测里到处都绿。开关的三种状态各断言一次：没接句柄 /
/// 接了但库里没有那一行（缺省关）/ 接了且打开。
#[tokio::test]
async fn run_command_rewrites_only_while_the_switch_is_on() {
    let fx = fixture().await;
    let (_, bin_dir) = pinned(&fx._home, "ls", "rtk marker");
    let fake = bin_dir.join("rtk");

    // ① 开关打开、但**没接读句柄**：不注入就不改写——这条能力从哪来，在构造点看得见
    fx.store.set_rtk_switch(true, Some(&fake)).await.unwrap();
    let (content, row) = run_once(&fx, false).await;
    assert!(
        !content.contains("SHIM-RAN"),
        "没接开关就绝不改写：{content}"
    );
    assert_eq!(row.command, "ls");
    assert_eq!(row.original_command, None);

    // ② 接了句柄、但库里**没有那一行**（缺省关）：行为逐字等于今天
    fx.store.clear_rtk_switch().await.unwrap();
    let (content, row) = run_once(&fx, true).await;
    assert!(!content.contains("SHIM-RAN"), "缺省关：{content}");
    assert_eq!(row.command, "ls");
    assert_eq!(row.original_command, None);
    // 关掉**不留残迹**（票 04）：shim 那条链接是「这台机器还在用 rtk」的假证据，
    // 二进制被卸掉之后它还是一条悬空链接。
    assert!(
        !agentpipeline_core::rtk::shim_dir(fx._home.home()).exists(),
        "关掉开关之后 shim 目录不该还在"
    );

    // ③ 接了句柄且打开：走 rtk，台账两份都在。
    //    改写 fail-open（2s 没回来就当没这回事），而满载时偶尔会撞上——故试到看见为止；
    //    每一次都过一遍台账两列的自洽，半改写的状态不许存在。
    fx.store.set_rtk_switch(true, Some(&fake)).await.unwrap();
    let mut rewritten = false;
    for _ in 0..REWRITE_ATTEMPTS {
        let (content, row) = run_once(&fx, true).await;
        ledger_is_consistent(&row);
        if row.original_command.is_some() {
            assert!(
                content.contains("SHIM-RAN marker"),
                "开关打开时该走 rtk：{content}"
            );
            assert_eq!(row.command, "rtk marker", "实际执行的是改写后的那条");
            assert_eq!(row.original_command.as_deref(), Some("ls"), "原串也留着");
            rewritten = true;
            break;
        }
    }
    assert!(
        rewritten,
        "连试 {REWRITE_ATTEMPTS} 次都没改写：开关没通到收口，或 shim 解析不到 rtk"
    );
}

/// 跑一次 `run_command "ls"`，回（给模型的回执文本，那一行的台账读数）。
async fn run_once(
    fx: &Fixture,
    with_store: bool,
) -> (String, agentpipeline_core::types::NodeCommand) {
    use agentpipeline_core::agent::client::ToolCall;
    use agentpipeline_core::agent::file_policy::FileToolPolicy;
    use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};

    let task_dir = fx._home.home().task_dir(&fx.task_id);
    std::fs::create_dir_all(&task_dir).unwrap();
    let mut executor = ToolExecutor::new(
        fx._home.home().clone(),
        FileToolPolicy::new(vec![fx.cwd.clone(), task_dir.clone()]),
        Default::default(),
        Arc::new(RealProcessKiller),
    )
    .with_recorder(Arc::new(fx.store.clone()));
    if with_store {
        executor = executor.with_rtk_store(fx.store.clone());
    }
    let ctx = ToolCallContext {
        task_id: fx.task_id.clone(),
        session_id: None,
        stage: Stage::Develop,
        node: Node::Execute,
        worktree_path: fx.cwd.clone(),
        task_dir,
        run_id: Some(fx.run_id),
        command_source: CommandSource::Agent,
        default_cwd: Some(fx.cwd.clone()),
    };
    let call = ToolCall {
        id: "c1".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command": "ls"}).to_string(),
    };
    let outcome = executor.execute(&call, &ctx).await.unwrap();
    let rows = fx
        .store
        .list_commands(&fx.task_id, None, None)
        .await
        .unwrap();
    let last = rows.last().cloned().expect("落过台账");
    (outcome.content, last)
}
