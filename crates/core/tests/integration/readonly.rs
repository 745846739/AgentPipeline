//! L2 集成：`run_readonly`——只读取证的白名单命令（决策 232 / 237，票 02）。
//!
//! 这个工具的安全面就是三条判定（决策 232 的原文要求「形状要写死，别用原样匹配这种含糊话
//! 混过去」），故用例逐条咬在**副作用**上而不是返回值上（与 `env_mode.rs` 同一取证手法）：
//!
//! 1. **不经 shell**：把 `; touch <文件>` 当参数递进去，文件必须**不存在**——只断言返回文本
//!    里没有某个字样是不够的（那可能只是被吃掉了）。
//! 2. **按命令名判定**：白名单外的名字（`sh` / `rm`）一律拒，且**留一行台账**（决策 179：
//!    被拒的尝试也要在审计面看得见）。
//! 3. **两条参数校验**：路径不得越出文件域（`data/` 与域外都要拒）、`sample` 的 pid 必须落
//!    「本服务的 pid + 其子进程」集合内。
//!
//! 另有一条钉「它不归档位管」（决策 237）：`deny` 档下它照旧广告、照旧执行——档位管的是
//! 「能不能改动东西」，而它改不了任何东西。

use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::{
    foreman_available_tools_except, FOREMAN_WATCH_TOOL_DENY,
};
use agentpipeline_core::types::{CommandSource, EnvMode, Node, Stage};
use agentpipeline_core::{Error, Result};
use testkit::{ManualClock, RecordingKiller, TestHome};

struct Fixture {
    _home: TestHome,
    store: agentpipeline_core::storage::Store,
    session_id: String,
}

async fn fixture() -> Fixture {
    let home = TestHome::new().unwrap();
    let store = agentpipeline_core::storage::Store::open(
        home.home().clone(),
        Arc::new(ManualClock::fixed()),
    )
    .await
    .unwrap();
    let session_id = store.create_foreman_session("只读取证").await.unwrap().id;
    Fixture {
        _home: home,
        store,
        session_id,
    }
}

impl Fixture {
    /// 值班长那一侧的构造（域 = 家目录根 + `data/` 前缀拒）——与 `foreman_tooling` 同源。
    fn executor(&self, mode: EnvMode) -> ToolExecutor {
        self.executor_with_killer(mode, Arc::new(RecordingKiller::new()))
    }

    /// 同 [`Fixture::executor`]，但交回终止器的把手——超时那条用例要看它有没有被叫到。
    fn executor_with_killer(&self, mode: EnvMode, killer: Arc<RecordingKiller>) -> ToolExecutor {
        ToolExecutor::new(
            self._home.home().clone(),
            agentpipeline_core::agent::file_policy::foreman_file_policy(self._home.home().root()),
            Settings::default(),
            killer,
        )
        .with_recorder(Arc::new(self.store.clone()))
        .with_env_mode(mode)
    }

    fn ctx(&self) -> ToolCallContext {
        ToolCallContext {
            task_id: String::new(),
            session_id: Some(self.session_id.clone()),
            stage: Stage::Init,
            node: Node::Execute,
            worktree_path: self._home.home().root().to_path_buf(),
            task_dir: self._home.home().root().to_path_buf(),
            run_id: None,
            command_source: CommandSource::Agent,
            default_cwd: None,
        }
    }
}

fn call(command: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: "run_readonly".into(),
        arguments: serde_json::json!({"command": command, "args": args}).to_string(),
    }
}

/// 台账里那一行（被拒的尝试也要有）。
async fn last_command(f: &Fixture) -> agentpipeline_core::types::NodeCommand {
    f.store
        .list_foreman_commands(&f.session_id)
        .await
        .unwrap()
        .pop()
        .expect("应当落了一行命令台账")
}

#[tokio::test]
async fn a_whitelisted_command_runs_without_a_shell() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Auto);
    let ctx = f.ctx();

    // 正面：白名单里的 `date` 真的跑起来了。
    let printed = executor
        .execute(&call("date", serde_json::json!(["+%Y"])), &ctx)
        .await
        .unwrap();
    let year: i32 = printed
        .content
        .trim()
        .parse()
        .expect("date 的输出应当是年份");
    assert!(year > 2000, "date 真的执行了：{}", printed.content);

    // 牙齿：`; touch <marker>` 作为**参数**递进去。若中间有一层 shell，它会执行并建出文件；
    // 而 argv 直出时它只是 `date` 的一个参数（`date` 会报错），文件**不该存在**。
    let marker = f._home.home().root().join("PWNED");
    let injected = format!("; touch {}", marker.display());
    executor
        .execute(&call("date", serde_json::json!(["+%s", injected])), &ctx)
        .await
        .unwrap();
    assert!(
        !marker.exists(),
        "分号没有落点——参数没有被 shell 解释（经 shell 的话 {} 会被建出来）",
        marker.display()
    );
}

#[tokio::test]
async fn commands_outside_the_whitelist_are_refused_before_running() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Auto);
    let ctx = f.ctx();

    for (program, args) in [
        ("sh", serde_json::json!(["-c", "echo x"])),
        ("rm", serde_json::json!(["-rf", "/"])),
        ("curl", serde_json::json!([])),
    ] {
        let error = executor
            .execute(&call(program, args), &ctx)
            .await
            .wrap_err();
        assert!(
            matches!(error, Error::PolicyDenied(_)),
            "{program} 应当被白名单拒掉，实际：{error:?}"
        );
        assert!(
            error.to_string().contains("白名单"),
            "报文要说清是白名单拒的：{error}"
        );
    }

    // 被拒的尝试**留在审计面上**（决策 179 的姿态）：最后一行是循环里最后一次尝试
    // （`curl`），且注明了原因、没有退出码（没有进程跑起来）。
    let row = last_command(&f).await;
    assert!(
        row.command.starts_with("curl"),
        "被拒的命令：{}",
        row.command
    );
    assert!(
        row.stderr_preview
            .as_deref()
            .unwrap_or("")
            .contains("白名单"),
        "拒绝的原因要在台账里：{:?}",
        row.stderr_preview
    );
    assert_eq!(row.exit_code, None, "没跑起来就没有退出码");
}

#[tokio::test]
async fn arguments_cannot_leave_the_readonly_file_domain() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Auto);
    let ctx = f.ctx();
    std::fs::create_dir_all(f._home.home().root().join("logs")).unwrap();
    std::fs::write(
        f._home.home().root().join("logs/agentpipeline.log"),
        "hello",
    )
    .unwrap();

    // 域**之内**的读是真能用的（否则这条工具没有意义）。
    let counted = executor
        .execute(
            &call("wc", serde_json::json!(["-c", "logs/agentpipeline.log"])),
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        counted.content.contains('5'),
        "5 字节的文件要数出 5：{}",
        counted.content
    );

    // `data/` 按前缀拒（库里明文存着 provider 密钥，决策 206 / 226）。
    let denied = executor
        .execute(
            &call(
                "tail",
                serde_json::json!(["-n", "1", "data/agentpipeline.db"]),
            ),
            &ctx,
        )
        .await
        .wrap_err();
    assert!(
        matches!(denied, Error::PolicyDenied(_)),
        "密钥库要读不到：{denied:?}"
    );

    // 带 `=` 的选项里那个值同样要过域（决策 232 的原文把「路径**与选项**」并列）：
    // `--files0-from=/etc/passwd` 这种写法不判就漏了。
    let option_path = executor
        .execute(
            &call("wc", serde_json::json!(["--files0-from=/etc/passwd"])),
            &ctx,
        )
        .await
        .wrap_err();
    assert!(
        matches!(option_path, Error::PolicyDenied(_)),
        "选项里带的路径也要过域：{option_path:?}"
    );
    // 域内的那一半照常放行（判定收的是路径，不是「有没有 `=`」）。
    let inside = executor
        .execute(
            &call(
                "wc",
                serde_json::json!(["--files0-from=logs/agentpipeline.log"]),
            ),
            &ctx,
        )
        .await;
    assert!(inside.is_ok(), "域内的选项值不该被拒：{inside:?}");

    // 域**之外**的路径同样拒（含 `..` 穿越与绝对路径两种写法）。
    for path in ["../outside.txt", "/etc/hosts"] {
        let denied = executor
            .execute(&call("tail", serde_json::json!(["-n", "1", path])), &ctx)
            .await
            .wrap_err();
        assert!(
            matches!(denied, Error::PolicyDenied(_)),
            "{path} 应当越界被拒：{denied:?}"
        );
    }
}

#[tokio::test]
async fn the_sample_pid_gate_covers_our_own_process_tree_only() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Auto);
    let ctx = f.ctx();

    // 别人的进程：拒（`sample` 是白名单里唯一能读走别的进程内存镜像的一个）。
    let denied = executor
        .execute(&call("sample", serde_json::json!(["1", "1", "1"])), &ctx)
        .await
        .wrap_err();
    assert!(matches!(denied, Error::PolicyDenied(_)), "{denied:?}");
    assert!(
        denied.to_string().contains("只允许对本服务的进程取证"),
        "报文要说清 pid 限定：{denied}"
    );

    // 按进程名取样：拒（那会绕过 pid 校验）。
    let by_name = executor
        .execute(&call("sample", serde_json::json!(["SomeName", "1"])), &ctx)
        .await
        .wrap_err();
    assert!(matches!(by_name, Error::PolicyDenied(_)), "{by_name:?}");
    assert!(
        by_name.to_string().contains("数字 pid"),
        "报文要说清只收 pid：{by_name}"
    );

    // 本进程自己：过闸（不因 pid 被拒——`sample` 本身能不能跑与这条判定无关）。
    let ours = std::process::id().to_string();
    let own = executor
        .execute(&call("sample", serde_json::json!([ours, "1", "1"])), &ctx)
        .await;
    match own {
        Ok(outcome) => assert!(
            !outcome.content.contains("只允许对本服务的进程取证"),
            "本进程自己不该被 pid 判定拒掉：{}",
            outcome.content
        ),
        Err(e) => assert!(
            !e.to_string().contains("只允许对本服务的进程取证"),
            "本进程自己不该被 pid 判定拒掉：{e}"
        ),
    }

    // 自己的**子进程**：也过闸（判据是「本服务的 pid + 其子进程」）。
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .expect("起一个子进程");
    let child_pid = child.id().to_string();
    let child_outcome = executor
        .execute(
            &call("sample", serde_json::json!([child_pid, "1", "1"])),
            &ctx,
        )
        .await;
    let _ = child.kill();
    let _ = child.wait();
    match child_outcome {
        Ok(outcome) => assert!(
            !outcome.content.contains("只允许对本服务的进程取证"),
            "子进程不该被 pid 判定拒掉：{}",
            outcome.content
        ),
        Err(e) => assert!(
            !e.to_string().contains("只允许对本服务的进程取证"),
            "子进程不该被 pid 判定拒掉：{e}"
        ),
    }
}

#[tokio::test]
async fn the_deny_tier_still_advertises_and_runs_it() {
    // 决策 237：档位管的是「**能不能改动东西**」（决策 206），而它改不了任何东西——
    // 故三档语义一个都不动，`deny` 档下照旧广告、照旧执行。这是「自主轮能取证」的地基。
    let f = fixture().await;
    let advertised = foreman_available_tools_except(EnvMode::Deny, &[]);
    assert!(
        advertised.contains(&"run_readonly"),
        "deny 档也要广告它：{advertised:?}"
    );
    assert!(
        !FOREMAN_WATCH_TOOL_DENY.contains(&"run_readonly"),
        "值守轮的 deny 清单不该拦它（拦了就没有自主取证这回事了）"
    );

    let executor = f.executor(EnvMode::Deny);
    let printed = executor
        .execute(&call("date", serde_json::json!(["+%Y"])), &f.ctx())
        .await
        .expect("deny 档下它照旧执行（它不改动任何东西）");
    assert!(!printed.content.trim().is_empty());
}

/// `Result` 的错误侧取出来（被拒是这块的主要断言对象）。
trait WrapErr<T> {
    fn wrap_err(self) -> Error;
}

impl<T> WrapErr<T> for Result<T> {
    fn wrap_err(self) -> Error {
        match self {
            Ok(_) => panic!("期望被拒，实际成功"),
            Err(e) => e,
        }
    }
}

/// 超时**要把进程组杀掉**（决策 66 / 票 17 的同一姿态）——这条从 `run_command` 那条路上
/// 抽出来时曾经**漏在只读这一支**。
///
/// 为什么单列：`run_readonly` 原本是把 `run_command` 的那一段抄一遍，抄的时候少了一句
/// 「超时后补一次 `kill_process_group`」。后果是取证命令超时之后进程组留到天荒地老——
/// 而这份白名单里正好有 `sample`（能读走内存镜像的那一个）。现在两条路共用同一条管道
/// （`run_child_to_outcome`），这条用例钉住「共用之后它真的在管道里」。
#[tokio::test]
async fn a_timed_out_readonly_command_kills_its_process_group() {
    let f = fixture().await;
    let killer = Arc::new(RecordingKiller::new());
    let executor = f.executor_with_killer(EnvMode::Auto, killer.clone());
    let mut ctx = f.ctx();
    // 一条会一直挂着的白名单命令（`tail -f` 一个没人写的文件），超时给 1 秒。
    let spin = f._home.home().root().join("quiet.log");
    std::fs::write(&spin, "").unwrap();
    ctx.stage = Stage::Test; // 无关紧要，只为让 timeout 走 test 那一档之外

    let out = executor
        .execute(
            &ToolCall {
                id: "c".into(),
                name: "run_readonly".into(),
                arguments: serde_json::json!({
                    "command": "tail",
                    "args": ["-f", spin.display().to_string()],
                    "timeout_sec": 1
                })
                .to_string(),
            },
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        out.content.contains("命令超时"),
        "超时要如实报出来，不是静默截断：{}",
        out.content
    );
    // 两次：超时当时一次 + 收口时又确认一次（`run_command` 那一支原本就有两次，
    // 抽管道时把它一并带过来了）。关键是**至少一次**，且 pgid 不是 0。
    let killed = killer.killed_groups();
    assert!(
        !killed.is_empty(),
        "超时必须杀进程组（这份白名单里有 sample，能读内存镜像的那个）"
    );
    assert!(
        killed.iter().all(|pgid| *pgid > 0),
        "杀的是真 pgid，不是 0（kill(0) 是 no-op）：{killed:?}"
    );
    // 台账要留下那一行（超时的命令也跑过）
    let last = last_command(&f).await;
    assert!(last.command.contains("tail"), "{}", last.command);
    assert!(last.exit_code.is_none(), "超时的命令没有退出码可填");
}
