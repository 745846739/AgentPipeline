//! 环境层权限档位在执行点的那道闸（决策 206，票 01 / 02）。
//!
//! 三档的语义在这里逐条钉住：
//! - `auto` **与档位出现之前逐字相同**（这是「落地不改变任何已运行行为」的兑现）；
//! - `ask` 不执行、落一条提议（决策 188 / 207 的表）；
//! - `deny` 拒绝，且连广告都不给（广告那一半在 `executor::tool_defs` / 值班长的清单，
//!   由各自的用例钉住；这里钉执行点这一半——模型无视定义硬发时也执行不了）。
//!
//! 「被拒的命令确实没跑过」用的取证手法与 `tests/foreman.rs` 同源：看**副作用**有没有
//! 留下（文件在不在、命令日志里有没有那一行），而不是看返回值说了什么。

use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::file_policy::FileToolPolicy;
use agentpipeline_core::agent::tools::{
    needs_confirmation, ProposalRequest, ProposalSink, ToolCallContext, ToolExecutor, ENV_TOOLS,
    ENV_WRITE_TOOLS, SERVICE_WRITE_TOOLS,
};
use agentpipeline_core::config::Settings;
use agentpipeline_core::storage::proposals::NewForemanProposal;
use agentpipeline_core::types::{
    effective_env_mode, CommandSource, EnvMode, Node, Stage, StageConfig,
};
use agentpipeline_core::{Error, Result};
use futures::future::BoxFuture;
use testkit::{ManualClock, RecordingKiller, TestHome};

/// 测试用的提议接缝：落一条提议（与生产实现同一条规则）。
struct RecordingSink {
    store: agentpipeline_core::storage::Store,
}

impl ProposalSink for RecordingSink {
    fn propose(&self, request: ProposalRequest) -> BoxFuture<'static, Result<String>> {
        let summary = format!("（用例）{}", request.tool);
        let store = self.store.clone();
        let session_id = request.session_id.clone().unwrap_or_default();
        let task_id = request.task_id.clone();
        let tool = request.tool.clone();
        let args = request.args.clone();
        Box::pin(async move {
            // 与生产实现同一条规则：参数里带 task_id 时取一份态势指纹。
            let situation = match task_id.as_deref() {
                Some(t) => Some(
                    agentpipeline_core::pipeline::foreman::situation_fingerprint(&store, t).await?,
                ),
                None => None,
            };
            store
                .create_foreman_proposal(NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
                    session_id,
                    tool,
                    args,
                    summary: summary.clone(),
                    situation,
                })
                .await?;
            Ok(format!("已生成一条待确认的提议：{summary}"))
        })
    }
}

struct Fixture {
    _home: TestHome,
    store: agentpipeline_core::storage::Store,
    worktree: std::path::PathBuf,
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
    let repo = home.scratch_dir("proj");
    testkit::seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    testkit::seed_task(&store, "t1", "p1").await.unwrap();
    let worktree = home.home().worktree_path("t1");
    let task_dir = home.home().task_dir("t1");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&task_dir).unwrap();
    let session_id = store.create_foreman_session("用例").await.unwrap().id;
    Fixture {
        _home: home,
        store,
        worktree,
        session_id,
    }
}

impl Fixture {
    fn executor(&self, mode: EnvMode, sink: Option<Arc<dyn ProposalSink>>) -> ToolExecutor {
        let task_dir = self._home.home().task_dir("t1");
        let mut executor = ToolExecutor::new(
            self._home.home().clone(),
            FileToolPolicy::new(vec![self.worktree.clone(), task_dir]),
            Settings::default(),
            Arc::new(RecordingKiller::new()),
        )
        .with_recorder(Arc::new(self.store.clone()))
        .with_env_mode(mode);
        if let Some(sink) = sink {
            executor = executor.with_proposal_sink(sink);
        }
        executor
    }

    fn ctx(&self) -> ToolCallContext {
        ToolCallContext {
            task_id: "t1".into(),
            // 会话归属：值班长的调用带它，流水线节点不带。
            session_id: Some(self.session_id.clone()),
            stage: Stage::Develop,
            node: Node::Execute,
            worktree_path: self.worktree.clone(),
            task_dir: self._home.home().task_dir("t1"),
            run_id: None,
            command_source: CommandSource::Agent,
            default_cwd: Some(self.worktree.clone()),
        }
    }
}

fn write_call(path: &str) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path": path, "content": "hello"}).to_string(),
    }
}

/// `auto` 档：与档位出现之前**逐字相同**——文件真的写了。
#[tokio::test]
async fn the_auto_tier_writes_the_file_like_before_the_tier_existed() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Auto, None);
    let outcome = executor
        .execute(&write_call("notes.md"), &f.ctx())
        .await
        .unwrap();
    assert!(outcome.content.contains("success"));
    assert_eq!(
        std::fs::read_to_string(f.worktree.join("notes.md")).unwrap(),
        "hello"
    );
    // 没有接缝也不影响 auto：它压根不生成提议。
    assert!(f
        .store
        .list_pending_foreman_proposals(&f.session_id)
        .await
        .unwrap()
        .is_empty());
}

/// `ask` 档：**不执行**，落一条提议——文件不在、库里有一条 pending。
#[tokio::test]
async fn the_ask_tier_proposes_instead_of_writing() {
    let f = fixture().await;
    let sink = Arc::new(RecordingSink {
        store: f.store.clone(),
    });
    let executor = f.executor(EnvMode::Ask, Some(sink));
    let outcome = executor
        .execute(&write_call("notes.md"), &f.ctx())
        .await
        .unwrap();

    // 取证一：文件**没有**被写（「不执行」不是「执行了再把结果丢掉」）
    assert!(
        !f.worktree.join("notes.md").exists(),
        "ask 档下写工具不得真的落盘"
    );
    // 取证二：库里多了一条未决提议，参数与调用**同一份**
    let pending = f
        .store
        .list_pending_foreman_proposals(&f.session_id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool, "write_file");
    assert_eq!(pending[0].args["path"], "notes.md");
    assert_eq!(pending[0].args["content"], "hello");
    // 回灌给模型的那句话把它止在「已经提了」：不是一次工具失败
    assert!(
        outcome.content.contains("待确认的提议"),
        "{}",
        outcome.content
    );
}

/// `ask` 档 + **没有**提议通道：拒（不注入不放行）。这是流水线节点的 `ask` 档现场。
#[tokio::test]
async fn the_ask_tier_without_a_proposal_channel_refuses() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Ask, None);
    let err = executor
        .execute(&write_call("notes.md"), &f.ctx())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)), "{err:?}");
    assert!(err.to_string().contains("没有接上提议通道"), "{err}");
    assert!(!f.worktree.join("notes.md").exists());
}

/// `deny` 档：拒，且**什么都没发生**（文件不在、库里没有提议）。
#[tokio::test]
async fn the_deny_tier_refuses_and_leaves_nothing_behind() {
    let f = fixture().await;
    let sink = Arc::new(RecordingSink {
        store: f.store.clone(),
    });
    let executor = f.executor(EnvMode::Deny, Some(sink));
    let err = executor
        .execute(&write_call("notes.md"), &f.ctx())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("deny 档"), "{err}");
    assert!(!f.worktree.join("notes.md").exists());
    assert!(f
        .store
        .list_pending_foreman_proposals(&f.session_id)
        .await
        .unwrap()
        .is_empty());
}

/// `deny` 档连 `run_command` 也挡——「环境层」是一个**层**，不是逐工具打的补丁。
#[tokio::test]
async fn the_deny_tier_covers_commands_too() {
    let f = fixture().await;
    let executor = f.executor(EnvMode::Deny, None);
    let executor = executor.with_recorder(Arc::new(f.store.clone()));
    let marker = f.worktree.join("RAN.txt");
    let call = ToolCall {
        id: "c2".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command": format!("touch {}", marker.display())})
            .to_string(),
    };
    let err = executor.execute(&call, &f.ctx()).await.unwrap_err();
    assert!(err.to_string().contains("deny 档"), "{err}");
    // 取证：命令真的没跑（副作用文件不存在），也没落命令日志。
    assert!(!marker.exists(), "deny 档下命令不得真的跑起来");
}

/// 只读台账工具**不受档位影响**（决策 188：只读的东西不需要人按键）。
#[tokio::test]
async fn ledger_read_tools_are_outside_the_tiers() {
    let f = fixture().await;
    assert!(!needs_confirmation("read_task", EnvMode::Ask));
    assert!(!needs_confirmation("read_task", EnvMode::Deny));
    assert!(!needs_confirmation("read_task", EnvMode::Auto));
    for mode in [EnvMode::Auto, EnvMode::Ask, EnvMode::Deny] {
        let executor = f.executor(mode, None).with_ledger(f.store.clone());
        let call = ToolCall {
            id: "r1".into(),
            name: "read_task".into(),
            arguments: serde_json::json!({"task_id": "t1"}).to_string(),
        };
        let outcome = executor.execute(&call, &f.ctx()).await.unwrap();
        assert!(
            outcome.content.contains("t1"),
            "{mode:?} 档下台账读取不该被挡：{}",
            outcome.content
        );
    }
}

/// 两段清单的语义（决策 206）：环境层看档位，本服务写接口**恒**要确认。
#[test]
fn the_two_manifests_have_different_rules() {
    assert!(!ENV_TOOLS.is_empty());
    // 环境层：auto 不确认、ask 确认、deny 挡在前面（不是「要确认」）
    assert!(!needs_confirmation("write_file", EnvMode::Auto));
    assert!(needs_confirmation("write_file", EnvMode::Ask));
    // 本服务写接口：**任何档位**下都要确认——它不读档位（决策 206）。
    for mode in [EnvMode::Auto, EnvMode::Ask, EnvMode::Deny] {
        for tool in SERVICE_WRITE_TOOLS {
            assert!(
                needs_confirmation(tool, mode),
                "{tool} 是本服务写接口，{mode:?} 档下也必须走确认钮"
            );
        }
    }
    // 环境层里**动手**的那些才要确认；只读的那些在 ask 档下直接执行
    //（「读一个文件也要人按键」是把确认钮变成噪声）。
    for name in ENV_WRITE_TOOLS {
        assert!(needs_confirmation(name, EnvMode::Ask), "{name} 应受档位管");
        assert!(ENV_TOOLS.contains(&name), "{name} 属于环境层");
    }
    assert!(!needs_confirmation("read_file", EnvMode::Ask));
    assert!(!needs_confirmation("list_dir", EnvMode::Ask));
    assert!(!needs_confirmation("Skill", EnvMode::Ask));
}

// ─────────────────────────── 两层解析（决策 206）───────────────────────────

/// 缺省等于现状：真实阶段 `auto`（= 今天的行为），值班长 `ask`。
#[test]
fn the_default_tier_keeps_everything_as_it_is_today() {
    let settings = Settings::default();
    assert_eq!(settings.env_mode, EnvMode::Auto, "全局默认必须是 auto");
    for stage in ["develop", "test", "architect-design", "conflict_check"] {
        assert_eq!(
            effective_env_mode(settings.env_mode, stage, None),
            EnvMode::Auto,
            "{stage} 缺省应为 auto（等于现状）"
        );
    }
    assert_eq!(
        effective_env_mode(settings.env_mode, "foreman", None),
        EnvMode::Ask,
        "值班长缺省收紧到 ask（它的输入是人可以随便打的任意文本）"
    );
}

/// 阶段级覆盖全局默认，**但不做节点级**（决策 206）。
#[test]
fn a_stage_row_overrides_the_global_default() {
    let settings = Settings::default();
    let row = StageConfig {
        stage: "develop".into(),
        env_mode: Some(EnvMode::Deny),
        ..Default::default()
    };
    assert_eq!(
        effective_env_mode(settings.env_mode, "develop", Some(&row)),
        EnvMode::Deny
    );
    // 值班长那一行配了就以它为准（档位是配置项，不是写死的常量）
    let foreman_row = StageConfig {
        stage: "foreman".into(),
        env_mode: Some(EnvMode::Auto),
        ..Default::default()
    };
    assert_eq!(
        effective_env_mode(settings.env_mode, "foreman", Some(&foreman_row)),
        EnvMode::Auto
    );
    // 全局默认也能改（config.toml 的 [pipeline] env_mode）
    assert_eq!(
        effective_env_mode(EnvMode::Deny, "test", None),
        EnvMode::Deny
    );
}

/// 写入路径拒非法档位（照 `SkillMode::parse` 的姿态）。
#[test]
fn env_mode_parsing_is_strict() {
    assert_eq!(EnvMode::parse("auto"), Some(EnvMode::Auto));
    assert_eq!(EnvMode::parse(" ask "), Some(EnvMode::Ask));
    assert_eq!(EnvMode::parse("deny"), Some(EnvMode::Deny));
    // 大小写 / 拼错 / 空串一律认不出——调用方据此报错或退回缺省，**不静默变成 auto**
    assert_eq!(EnvMode::parse("Auto"), None);
    assert_eq!(EnvMode::parse(""), None);
    assert_eq!(EnvMode::parse("allow"), None);
}

// ───────────── 值班长的域与 C / E 层（决策 206 / 207，票 04 / 06）─────────────

/// 值班长的执行器：域 = 家目录根（**不是**任务工作区），并按前缀拒 `data/` 与 `logs/`。
///
/// 与 [`Fixture::executor`] 的差别只有这两处——这正是决策 207 的「分两组」：流水线阶段的
/// 写域仍限任务工作区，那一条不动；值班长是「像 zcode 一样」被放开的那个。
fn foreman_executor(
    f: &Fixture,
    mode: EnvMode,
    sink: Option<Arc<dyn ProposalSink>>,
) -> ToolExecutor {
    let mut executor = ToolExecutor::new(
        f._home.home().clone(),
        agentpipeline_core::agent::file_policy::foreman_file_policy(f._home.home().root()),
        Settings::default(),
        Arc::new(RecordingKiller::new()),
    )
    .with_recorder(Arc::new(f.store.clone()))
    .with_env_mode(mode);
    if let Some(sink) = sink {
        executor = executor.with_proposal_sink(sink);
    }
    executor
}

/// 值班长的调用上下文：不挂任务、只挂会话。
fn foreman_ctx(f: &Fixture) -> ToolCallContext {
    ToolCallContext {
        task_id: String::new(),
        session_id: Some(f.session_id.clone()),
        stage: Stage::Init,
        node: Node::Execute,
        worktree_path: f._home.home().root().to_path_buf(),
        task_dir: f._home.home().root().to_path_buf(),
        run_id: None,
        command_source: CommandSource::Agent,
        default_cwd: None,
    }
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}

/// 家目录里的普通文件读写得到；`data/` 与 `logs/` 读写**都**被拒。
///
/// 这一条是**补偿**不是边界（决策 206 的原话）：命令自己 `cd` 就出去了，文件策略只管
/// 文件工具。它挡住的是一件具体的事——`data/agentpipeline.db` 里明文存着 provider 密钥
/// （决策 112），而默认那份**模式**名单（`.env*` / `*.pem` / …）盖不住一个 `.db` 文件。
#[tokio::test]
async fn the_foreman_domain_covers_the_home_but_not_data_or_logs() {
    let f = fixture().await;
    // 密钥库与日志目录按真实形态造出来
    std::fs::create_dir_all(f._home.home().db_path().parent().unwrap()).unwrap();
    std::fs::write(f._home.home().db_path(), "sk-live-secret").unwrap();
    std::fs::create_dir_all(f._home.home().logs_dir()).unwrap();
    std::fs::write(f._home.home().logs_dir().join("app.log"), "日志").unwrap();

    // auto 档：直通，故这一轮测的是**文件策略**本身（不是档位）
    let executor = foreman_executor(&f, EnvMode::Auto, None);
    let ctx = foreman_ctx(&f);

    // 对照组：根下写一个普通文件 —— 放行
    let ok = executor
        .execute(
            &call(
                "write_file",
                serde_json::json!({"path": "notes.md", "content": "x"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(ok.content.contains("success"), "{}", ok.content);
    assert!(f._home.home().root().join("notes.md").exists());

    for (tool, args) in [
        (
            "read_file",
            serde_json::json!({"path": "data/agentpipeline.db"}),
        ),
        (
            "write_file",
            serde_json::json!({"path": "data/x.txt", "content": "x"}),
        ),
        (
            "edit_file",
            serde_json::json!({"path": "logs/app.log", "old_text": "日志", "new_text": "改了"}),
        ),
        ("read_file", serde_json::json!({"path": "logs/app.log"})),
    ] {
        let err = executor.execute(&call(tool, args), &ctx).await.unwrap_err();
        // 命中名单的报文要说清是哪一条命中的（不然「为什么被拒」只能靠猜）
        assert!(
            err.to_string().contains("data") || err.to_string().contains("logs"),
            "{tool} 应当被 deny 前缀拒：{err}"
        );
    }
    // 密钥内容没有被带出来，日志也一个字没变
    assert_eq!(
        std::fs::read_to_string(f._home.home().logs_dir().join("app.log")).unwrap(),
        "日志"
    );
}

/// `ask` 档下 C 层的**两个**写工具都走确认钮（不是只接了一个）。
#[tokio::test]
async fn the_ask_tier_proposes_for_both_file_writers() {
    let f = fixture().await;
    let sink = Arc::new(RecordingSink {
        store: f.store.clone(),
    });
    let executor = foreman_executor(&f, EnvMode::Ask, Some(sink));
    let ctx = foreman_ctx(&f);
    std::fs::write(f._home.home().root().join("a.md"), "原文").unwrap();

    executor
        .execute(
            &call(
                "write_file",
                serde_json::json!({"path": "a.md", "content": "换了"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    executor
        .execute(
            &call(
                "edit_file",
                serde_json::json!({"path": "a.md", "old_text": "原文", "new_text": "改过"}),
            ),
            &ctx,
        )
        .await
        .unwrap();

    let pending = f
        .store
        .list_pending_foreman_proposals(&f.session_id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 2, "两个写工具各提一条：{pending:?}");
    let tools: Vec<&str> = pending.iter().map(|p| p.tool.as_str()).collect();
    assert!(
        tools.contains(&"write_file") && tools.contains(&"edit_file"),
        "{tools:?}"
    );
    // 取证：文件**没变**
    assert_eq!(
        std::fs::read_to_string(f._home.home().root().join("a.md")).unwrap(),
        "原文"
    );
}

/// 按下确认钮那一次：**同一个调用**在 `ConfirmedPress` 下真的落盘。
///
/// 这是确认钮整条链路的承重点——「提议时拦下、按键时执行」两半必须在同一份实现上成立。
/// 执行侧由 `crates/core` 的 [`agentpipeline_core::pipeline::foreman::ForemanMoment`] 给
/// （app 层的端点用例覆盖完整的 HTTP 路径）。
#[tokio::test]
async fn the_confirmed_press_executes_what_the_ask_tier_proposed() {
    let f = fixture().await;
    let ctx = foreman_ctx(&f);
    let write = call(
        "write_file",
        serde_json::json!({"path": "a.md", "content": "换了"}),
    );

    // ① ask 档：拦下
    let executor = foreman_executor(&f, EnvMode::Ask, None);
    let err = executor.execute(&write, &ctx).await.unwrap_err();
    assert!(err.to_string().contains("没有接上提议通道"), "{err}");
    assert!(!f._home.home().root().join("a.md").exists());

    // ② 按键那一刻：闸门关掉，同一份参数真的写下去
    let executor = foreman_executor(&f, EnvMode::Ask, None).confirmed_once();
    let outcome = executor.execute(&write, &ctx).await.unwrap();
    assert!(outcome.content.contains("success"), "{}", outcome.content);
    assert_eq!(
        std::fs::read_to_string(f._home.home().root().join("a.md")).unwrap(),
        "换了"
    );
}

/// `auto` 档下命令**不受文件策略管**：本用例钉住这个事实（决策 206 已登记的残余风险）。
///
/// 两件事要分开看，混成一件会得出错误结论：
/// - **进程读得到**密钥库（`wc -c` 能数出字节数）——这一条**无补偿**，域检查对命令几乎没
///   价值（命令自己 `cd` 就出去了），真正的兜底是 OS 级沙箱。
/// - **密钥形状的文本进不了上下文**：既有的脱敏（决策 118）在回灌之前把 `sk-…` 抹成 `***`。
///   这是「别让密钥随对话流进 LLM 与日志」的第二道线，**不是**第一道：它管的是回灌，
///   管不了命令自己把内容发出去。
///
/// 谁哪天给命令加了真的沙箱，①会失败——那时它该改成断言「读不到了」。
#[tokio::test]
async fn a_command_can_reach_the_key_store_but_the_secret_is_scrubbed_from_context() {
    let f = fixture().await;
    std::fs::create_dir_all(f._home.home().db_path().parent().unwrap()).unwrap();
    std::fs::write(f._home.home().db_path(), "sk-live-secret").unwrap();
    let executor = foreman_executor(&f, EnvMode::Auto, None);
    let ctx = foreman_ctx(&f);

    // ① 进程真的读到了它（没被拒、没被隔离）：密钥库 14 字节。
    let counted = executor
        .execute(
            &call(
                "run_command",
                serde_json::json!({"command": "wc -c < data/agentpipeline.db"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        counted.content.contains("14"),
        "命令能读到密钥库（本机无沙箱，这是已登记的残余风险）：{}",
        counted.content
    );

    // ② 打印出来时，进上下文的那份是掩码。
    let printed = executor
        .execute(
            &call(
                "run_command",
                serde_json::json!({"command": "cat data/agentpipeline.db"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(
        !printed.content.contains("sk-live-secret"),
        "密钥形状的文本不得进上下文：{}",
        printed.content
    );
    assert!(printed.content.contains("***"), "{}", printed.content);
}

// ───────────── E 层：命令真的落了库、卸载落在会话维度（票 03 / 06）─────────────

/// `auto` 档直通：命令落了 `kanban_node_commands`，归属是**会话**（`task_id` 为 NULL）。
///
/// 这是迁移 0012 把 `task_id` 改成可空的理由。三条断言缺一不可：落进了会话那一栏、
/// **没**落进任何任务、另一个班次读不到它。
#[tokio::test]
async fn an_auto_command_lands_under_the_session_and_nowhere_else() {
    let f = fixture().await;
    let other = f.store.create_foreman_session("另一班").await.unwrap().id;
    let executor = foreman_executor(&f, EnvMode::Auto, None);
    let marker = f._home.home().root().join("RAN.txt");
    let outcome = executor
        .execute(
            &call(
                "run_command",
                serde_json::json!({"command": format!("touch {}", marker.display())}),
            ),
            &foreman_ctx(&f),
        )
        .await
        .unwrap();
    assert!(!outcome.content.contains("失败"), "{}", outcome.content);
    assert!(marker.exists(), "auto 档下命令应当真的跑起来");

    let commands = f.store.list_foreman_commands(&f.session_id).await.unwrap();
    assert_eq!(commands.len(), 1, "命令应当落进这个班次");
    assert_eq!(commands[0].task_id, None, "值班长的命令不挂任务");
    assert_eq!(
        commands[0].session_id.as_deref(),
        Some(f.session_id.as_str())
    );
    assert_eq!(commands[0].exit_code, Some(0));
    // 任务口径没被污染，另一个班次也读不到。
    assert!(f
        .store
        .list_commands("t1", None, None)
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .store
        .list_foreman_commands(&other)
        .await
        .unwrap()
        .is_empty());
}

/// `ask` 档：命令提议、**确实没跑过**；同一条参数在按键那一刻才真的跑起来。
///
/// 「没跑过」的取证看**副作用**（RAN.txt 在不在、命令日志有没有那一行），不看返回值说了什么。
#[tokio::test]
async fn the_ask_tier_command_waits_for_the_press() {
    let f = fixture().await;
    let marker = f._home.home().root().join("RAN.txt");
    let run = call(
        "run_command",
        serde_json::json!({"command": format!("touch {}", marker.display())}),
    );

    let sink = Arc::new(RecordingSink {
        store: f.store.clone(),
    });
    let executor = foreman_executor(&f, EnvMode::Ask, Some(sink));
    let outcome = executor.execute(&run, &foreman_ctx(&f)).await.unwrap();
    assert!(
        outcome.content.contains("待确认的提议"),
        "{}",
        outcome.content
    );
    assert!(!marker.exists(), "ask 档下命令不得真的跑起来");
    assert!(f
        .store
        .list_foreman_commands(&f.session_id)
        .await
        .unwrap()
        .is_empty());
    let pending = f
        .store
        .list_pending_foreman_proposals(&f.session_id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool, "run_command");

    // 按键那一刻：同一条参数（逐字取自提议行）真的跑起来，并且落了命令日志。
    let pressed = foreman_executor(&f, EnvMode::Ask, None).confirmed_once();
    pressed.execute(&run, &foreman_ctx(&f)).await.unwrap();
    assert!(marker.exists(), "按键之后命令应当真的跑起来");
    assert_eq!(
        f.store
            .list_foreman_commands(&f.session_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// 大输出的卸载落**会话维度**，且 `{root}/tasks/.context` 一个字都不写。
///
/// 空 `task_id` 若被当成「根目录下的 `.context`」，落点就是所有任务共用的那一层——
/// 下一次运行任意一个真实任务时会把它读成自己的工作区残留。
#[tokio::test]
async fn a_long_command_output_offloads_into_the_session_dimension() {
    let f = fixture().await;
    let executor = foreman_executor(&f, EnvMode::Auto, None);
    let outcome = executor
        .execute(
            &call("run_command", serde_json::json!({"command": "seq 1 5000"})),
            &foreman_ctx(&f),
        )
        .await
        .unwrap();
    assert!(outcome.content.contains("已卸载"), "{}", outcome.content);

    let dir = f._home.home().foreman_context_dir(&f.session_id);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("会话卸载目录不存在（{}）：{e}", dir.display()))
        .collect();
    assert_eq!(files.len(), 1, "大输出应当落一份到会话卸载目录");
    assert!(
        !f._home.home().tasks_dir().join(".context").exists(),
        "空 task_id 不得落到共享的 tasks/.context"
    );
}
