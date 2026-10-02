//! 命令执行的**唯一收口**（决策 297 / 票 01–02）。
//!
//! 「启动一条命令 → 流式采集 → 超时收口 → 脱敏 → 台账回填」这条生命周期只在这里有一份。
//! 宿主是一个**不依赖 `ToolExecutor`** 的独立结构：闸门那侧手里只有 `Store` + `Settings`
//! （没有 `ToolExecutor`），而闸门也要走这条管道。
//!
//! 三个调用点（`run_command` / `run_readonly` / 两处闸门）由此一并拿到四件事：
//! **独立进程组 + 运行期心跳 + 超时杀干净 + 脱敏台账**。
//!
//! 为什么必须收口（而不是让改写各写一遍）：这条管道的每一步都有「不做会怎样」的后果，
//! 而两份实现**已经漂移过一次**——`run_command` 与 `run_readonly` 各写一遍时，只有一支在
//! 超时后补了杀进程组。闸门当时是第三、第四份，两处既有缺陷正是这么来的：repair 闸门
//! 连超时都没有（`.output().await` 裸调），runner 闸门超时只丢 future、不杀进程，且从不回填
//! `process_group_id`，于是调度器那条超时收口也够不着它。
//!
//! 两个刻意的取舍：
//!
//! - **不收 `Clock`**：时长用 `Instant` 量（启动到收口的墙钟），与两处既有实现同源，
//!   而 repair 闸门那条路上根本没有 clock 可传。台账的 `started_at` 仍由存储层自己取时。
//! - **`kill` / `ps` 不收**：它们是系统调用式的**探测**，不是「跑一条命令」（spec §1）。
//!
//! [`crate::process`] 保持「纯启动 + 终止」层，职责不动（决策 143 的接缝③）。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::sanitize::{sanitize_command_line, sanitize_text};
use crate::agent::tools::{CommandFinish, CommandRecorder, CommandSse, CommandStart};
use crate::process::{ChildEnv, ProcessKiller};
use crate::types::{CommandSource, Node, Stage};
use crate::Result;

/// 心跳默认周期：远小于 300s 空闲超时，600s 级测试命令也能存活（决策 100）。
pub const COMMAND_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// 单行推流上限（票 14 的节流策略之一）：超长行截断并标注，避免一行撑爆事件。
pub const STREAM_MAX_LINE_CHARS: usize = 4_000;
/// 单条命令最多推送的行数（票 14 的节流策略之二）：高频输出超过后停止推流并标注，
/// **完整输出仍全量缓冲**用于命令记录与回填——推流是观测面，不是数据来源。
pub const STREAM_MAX_LINES: usize = 2_000;

/// 改写策略（决策 297 / 票 02）。**同一个函数，挂不挂由调用点说清**。
///
/// 形态是一个参数而不是两个函数：这样「改写只可能挂在这一个函数上」成立，同时不把改写
/// 强加给不该改写的路径——闸门与 `run_readonly` 传 [`Rewrite::None`]，理由见 spec §2
/// （闸门输出同时是模型改代码的唯一证据与落盘取证物；`run_readonly` 的安全面是
/// 「命令名与参数是两个独立的数组元素」，把 `argv[0]` 换成 `rtk` 就丢了那个性质）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rewrite {
    /// 原样执行（缺省）。
    #[default]
    None,
    /// 执行前过一次 rtk 改写器（决策 297）。改写器不在场 / 没改 → 原样执行。
    Rtk,
}

/// 启动形态：两条路**只有这里不同**。
///
/// 这个差别不是形式上的——它就是决策 232 的安全面本身：argv 直出时命令名与参数是两个
/// 独立的数组元素，没有一层 shell 去解释分号、管道、`$(...)`。
pub enum SpawnForm<'a> {
    /// `sh -c <命令>`（`run_command` 与两处闸门）。
    Shell,
    /// argv 直出，不经 shell（`run_readonly`，决策 232 / 237）。
    Argv {
        program: &'a str,
        args: &'a [String],
    },
}

/// 台账「开始」那一行的**归属与上下文**（`command` / `cwd` / `original_command` 三列除外）。
///
/// 单独一个类型而不是直接收 [`CommandStart`]：后者的 `command` 一列是「**实际执行的**
/// 命令串（脱敏后）」，而那个值只有收口函数知道（改写发生在它内部）——让调用点先填一个
/// 会被覆盖的字段是给误读留位置。
#[derive(Debug, Clone)]
pub struct CommandOwner {
    /// 归属任务。值班长的命令给 `None`，改为挂 [`Self::session_id`]（决策 204④）。
    pub task_id: Option<String>,
    /// 归属会话（值班长的命令）。流水线命令为 `None`。
    pub session_id: Option<String>,
    pub run_id: Option<i64>,
    pub stage: Stage,
    pub node: Node,
    pub source: CommandSource,
}

impl CommandOwner {
    /// 从流水线 / 值班长的工具调用上下文取归属（`run_command` / `run_readonly` 那一侧）。
    pub fn from_ctx(ctx: &crate::agent::tools::ToolCallContext) -> Self {
        CommandOwner {
            task_id: Some(ctx.task_id.clone()),
            session_id: ctx.session_id.clone(),
            run_id: ctx.run_id,
            stage: ctx.stage,
            node: ctx.node,
            source: ctx.command_source,
        }
    }
}

/// 一次命令执行的请求：**除了「怎么记收尾那一行」以外的全部输入**。
pub struct CommandRequest<'a> {
    /// 台账归属。
    pub owner: CommandOwner,
    /// **原始**命令串（模型写的 / 项目配的），**未脱敏**——脱敏在落台账之前由本模块做。
    ///
    /// `SpawnForm::Shell` 时它就是拿去执行的那一条；`SpawnForm::Argv` 时它只是
    /// argv 拼回一行的**记录形态**（与 `run_command` 的 `command` 同一列）。
    pub command: &'a str,
    /// 子进程的工作目录。
    pub cwd: &'a Path,
    /// 超时上限（秒）——调用点算好（agent 侧走 `effective_run_command_timeout`，
    /// 闸门侧走 `test_command_timeout_sec`，决策 297：**复用既有键**，不新增）。
    pub timeout_sec: u64,
    pub spawn: SpawnForm<'a>,
    /// 改写策略。本模块只负责**执行**改写器给的串，改写本身在 [`crate::rtk`]。
    pub rewrite: Rewrite,
}

/// 一次命令执行的收口读数。输出**已脱敏**（决策 118）。
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// 台账那一行的 id（未接记录器时 `None`）。
    pub command_id: Option<i64>,
    /// 退出码；`None` = 没跑起来（启动失败 / 超时 / 被信号打死）。
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// 超时收口过：进程组已杀。
    pub timed_out: bool,
    pub duration_ms: u64,
}

impl CommandOutput {
    /// 进程**没跑起来**（可执行文件不在、权限不对之类）——与「跑起来了但超时」是两件事。
    ///
    /// 收口把三种结局收成一份读数（跑完 / 启动失败 / 超时），这一条是给调用点分流用的：
    /// 闸门把启动失败当**节点错误**、把超时当**闸门结果**（收口之前就是这两条路）。
    pub fn spawn_failed(&self) -> bool {
        self.exit_code.is_none() && !self.timed_out
    }
}

/// 改写的落点（决策 297 / 票 02）：真的发生过改写时，原串与执行串都要在台账上。
///
/// 换串的是一个**纯映射** [`crate::rtk::rewrite`]：收串吐串，失败一律 `None`（原样执行）。
struct Rewritten {
    /// 实际执行的命令串。
    exec: String,
    /// 模型原本写的串——**只有真的改写过**才是 `Some`。
    original: Option<String>,
}

/// 这一份 rtk 从哪来（决策 297 / 票 04）。
#[derive(Clone)]
enum RtkSource {
    /// 没有：改写与 PATH 前置都不发生（缺省）。
    Off,
    /// 固定一份（测试注入的假 rtk）。
    Pinned(Arc<crate::rtk::RtkRuntime>),
    /// **每次现读**库里的开关与手填路径——生产走这条，于是「保存即活」。
    ///
    /// 为什么现读而不是在构造点取快照：构造 `ToolExecutor` 的三条路里有一条
    /// （`foreman_tooling`）是**同步函数**，在那儿读库要把整条调用链改成 async。
    /// 现读的代价是每条命令两次小查询 + 一次 `read_link`（`pin` 在同一目标上直接返回，
    /// 不重建），相对一次进程启动可以忽略——换来的是改完开关下一条命令就生效。
    Store(crate::storage::Store),
}

/// 命令执行的收口宿主（决策 297）。
///
/// 一个裸结构而不是 `ToolExecutor` 的方法：闸门那侧只有 `Store` + `Settings`，手里没有
/// `ToolExecutor`。依赖全部靠字段显式注入，构造点即「它到底能碰什么」的清单。
pub struct CommandRunner {
    killer: Arc<dyn ProcessKiller>,
    recorder: Option<Arc<dyn CommandRecorder>>,
    /// 流式输出去向（决策 100 / §12.4.4）：按行推送命令输出。
    /// `None` = 不推流，行为与纯缓冲一致。
    sse: Option<CommandSse>,
    /// 运行期间的心跳周期（决策 100）；默认 5s，测试可调短。
    heartbeat_interval: Duration,
    rtk: RtkSource,
    /// 附加进每条命令子进程环境的环境变量（票 runner-offload/03）：
    /// 语义由调用点声明（如任务命令的 `CARGO_TARGET_DIR` → 共享构建缓存），
    /// 本层不解释、只在 [`Self::apply_rewrite`] 装配 [`ChildEnv`] 时透传。
    extra_env: Vec<(String, String)>,
}

impl CommandRunner {
    /// 构造点就是「它能碰什么」的清单，故只收它真的用得上的：终止器（超时杀进程组）。
    ///
    /// **不收 [`Settings`]**：超时上限由调用点按自己的语义算好传进来（agent 那侧是
    /// `effective_run_command_timeout`、闸门那侧是 `test_command_timeout_sec`），心跳周期
    /// 走 [`Self::with_heartbeat_interval`]——收一份 `Settings` 只会让收口有机会**自己**
    /// 重新解释一遍策略，而策略该由调用点说了算（spec §2：挂不挂改写、用哪个上限都由调用点声明）。
    pub fn new(killer: Arc<dyn ProcessKiller>) -> Self {
        CommandRunner {
            killer,
            recorder: None,
            sse: None,
            heartbeat_interval: COMMAND_HEARTBEAT_INTERVAL,
            rtk: RtkSource::Off,
            extra_env: Vec::new(),
        }
    }

    /// 附加子进程环境变量（票 runner-offload/03）。调用点声明语义，本层透传。
    pub fn with_extra_env(mut self, extra: Vec<(String, String)>) -> Self {
        self.extra_env = extra;
        self
    }

    pub fn with_recorder(mut self, recorder: Arc<dyn CommandRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    pub fn with_sse(mut self, sse: CommandSse) -> Self {
        self.sse = Some(sse);
        self
    }

    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = interval;
        self
    }

    /// 生产接线（票 04）：开局那一次从库里现读 rtk 的开关与手填路径。
    pub fn with_rtk_store(mut self, store: crate::storage::Store) -> Self {
        self.rtk = RtkSource::Store(store);
        self
    }

    /// 钉死一份运行期 rtk（测试的假 rtk，或调用点已经解析过的）。
    pub fn with_rtk(mut self, rtk: Option<Arc<crate::rtk::RtkRuntime>>) -> Self {
        self.rtk = match rtk {
            Some(rt) => RtkSource::Pinned(rt),
            None => RtkSource::Off,
        };
        self
    }

    /// 现取这一条命令要用的 rtk。
    ///
    /// 开关关着 / 找不到二进制 / shim 建不起来 → `None`：命令**原样执行**，
    /// 每个班次最多一条 `tracing::warn`（优化器不可用不该升级成整条命令失败）。
    async fn rtk_runtime(&self) -> Option<Arc<crate::rtk::RtkRuntime>> {
        match &self.rtk {
            RtkSource::Off => None,
            RtkSource::Pinned(rt) => Some(rt.clone()),
            RtkSource::Store(store) => {
                let switch = match store.rtk_switch().await {
                    Ok(s) => s,
                    Err(e) => {
                        crate::rtk::warn_unavailable(&format!("读开关失败：{e}"));
                        return None;
                    }
                };
                if !switch.enabled {
                    // 关掉要**不留残迹**（票 04）：留着那条链接是「这台机器还在用 rtk」的假
                    // 证据，二进制被卸掉之后它还是一条悬空链接。一次 stat（不存在时）的成本。
                    if let Err(e) = crate::rtk::unpin(store.home()) {
                        crate::rtk::warn_unavailable(&format!("清 shim 失败：{e}"));
                    }
                    return None;
                }
                crate::rtk::runtime(store.home(), true, switch.path.as_deref()).map(Arc::new)
            }
        }
    }

    /// 跑一条命令：**启动 → 流式采集 → 超时收口 → 脱敏 → 台账回填**。
    ///
    /// `finish` 在进程收口、输出脱敏之后被调用**恰好一次**：三处调用点的台账收尾各有各的
    /// `stdout_path` 语义（agent 侧是 L1/L2 之后的卸载路径，闸门是确定路径的全文日志），
    /// 故那三格由调用点填，并把它的附带产物（agent 侧要给模型的回执文本）一并带出来。
    /// 其余每一格——退出码 / 时长 / 心跳 / 进程组回填 / 超时后的第二次杀——都是同一条。
    pub async fn run<F, T>(&self, req: CommandRequest<'_>, finish: F) -> Result<(CommandOutput, T)>
    where
        F: FnOnce(&CommandOutput) -> Result<(CommandFinish, T)>,
    {
        // ① 改写（票 02）：**在落台账与启动之前**。判决顺序是不变量——
        //    `check(原命令) → 改写 → 落台账 → spawn`（出口检查在调用点，见 spec §6）。
        let (rewritten, env) = self.apply_rewrite(&req).await;

        // ② 台账「开始」+ 起始心跳（决策 179：被拒的与放行的走同一个入口）。
        let command_id = self.record_start(&req, &rewritten).await?;
        if let Some(rec) = &self.recorder {
            rec.touch_heartbeat(req.owner.run_id).await?;
        }

        self.execute(&req, &rewritten.exec, &env, command_id, finish)
            .await
    }

    /// 改写（决策 297 / 票 02），并给出这一条命令要用的子进程环境。
    ///
    /// 两种「不加前缀」要分清：
    /// - **不改写**（未启用 / 调用点传 `None` / rtk 不在场 / 改写器回了空）——对台账是同一件事，
    ///   都长成 `original_command IS NULL`（spec §6）。
    /// - **不改写但 PATH 照旧前置**：模型自己写了 `rtk ls` 时改写器按幂等回空串，
    ///   而那条 `rtk` 得靠 shim 找到。故只要 rtk 在场，前置就发生。
    async fn apply_rewrite(&self, req: &CommandRequest<'_>) -> (Rewritten, ChildEnv) {
        let untouched = || Rewritten {
            exec: req.command.to_string(),
            original: None,
        };
        let base_env = ChildEnv {
            path_prefix: None,
            extra_vars: self.extra_env.clone(),
        };
        if req.rewrite != Rewrite::Rtk {
            return (untouched(), base_env);
        }
        // argv 直出那一支永不改写（决策 232 的安全面）：这里再挡一次，而不是靠调用点自觉。
        if matches!(req.spawn, SpawnForm::Argv { .. }) {
            return (untouched(), base_env);
        }
        let Some(rtk) = self.rtk_runtime().await else {
            return (untouched(), base_env);
        };
        let env = ChildEnv {
            path_prefix: Some(rtk.shim_dir.clone()),
            extra_vars: self.extra_env.clone(),
        };
        match crate::rtk::rewrite(&rtk.binary, req.command).await {
            Some(rewritten) => (
                Rewritten {
                    exec: rewritten,
                    original: Some(req.command.to_string()),
                },
                env,
            ),
            // 不改写、但**照样前置 shim**：命令本身可能就是 `rtk …` 形态（改写器幂等，
            // 已带前缀的输入回空）。那时执行的串是原串，而它得靠 PATH 找到 rtk。
            None => (untouched(), env),
        }
    }

    async fn record_start(
        &self,
        req: &CommandRequest<'_>,
        rewritten: &Rewritten,
    ) -> Result<Option<i64>> {
        let Some(rec) = &self.recorder else {
            return Ok(None);
        };
        Ok(Some(
            rec.record_start(CommandStart {
                task_id: req.owner.task_id.clone(),
                session_id: req.owner.session_id.clone(),
                run_id: req.owner.run_id,
                stage: req.owner.stage,
                node: req.owner.node,
                source: req.owner.source,
                // 脱敏在**写入之前**（§12.4.4）：原串与执行串都过一遍——模型随手写的
                // 命令里可能有 token，而改写把它换了之后，两份都还是它写的（决策 118）。
                command: sanitize_command_line(&rewritten.exec),
                cwd: req.cwd.display().to_string(),
                original_command: rewritten.original.as_deref().map(sanitize_command_line),
            })
            .await?,
        ))
    }

    /// 启动 → 采集 → 超时收口 → 脱敏 → 台账收尾。这一段与「怎么启动」无关，
    /// 这正是它被抽出来的理由（票 01 的 Comments：两处各写一遍已经漂移过一次）。
    async fn execute<F, T>(
        &self,
        req: &CommandRequest<'_>,
        command: &str,
        env: &ChildEnv,
        command_id: Option<i64>,
        finish: F,
    ) -> Result<(CommandOutput, T)>
    where
        F: FnOnce(&CommandOutput) -> Result<(CommandFinish, T)>,
    {
        // 决策 100：运行期间周期心跳——600s 级命令不被 300s 空闲超时误杀
        let mut heartbeat = Heartbeat(self.spawn_command_heartbeat(req.owner.run_id));
        let started = Instant::now();
        // 独立进程组启动（票 17 / 决策 66）：捕获真实 pgid 回填 node_runs，
        // 超时回调终止器杀整个进程组（此前 kill(0) 是 no-op）。
        let mut child_pgid: Option<i32> = None;
        // 逐行读 + 按行推流（票 14 / 决策 100 / §12.4.4）：完整内容全量缓冲用于落库与回填。
        let collected = Arc::new(std::sync::Mutex::new(CollectedOutput::default()));

        let output = match self.spawn(req, command, env) {
            Ok(child) => {
                child_pgid = child.id().map(|id| id as i32);
                // 回填失败不能让已 spawn 的 `Child` 被丢掉（全仓没有 `kill_on_drop`，
                // 丢下它等于留一个没人回收的进程树）：`ChildGuard` 在这里保底。
                let mut guard = ChildGuard::new(child);
                let backfilled = match (self.recorder.as_ref(), req.owner.run_id, child_pgid) {
                    (Some(rec), Some(run_id), Some(pgid)) => {
                        rec.set_process_group(run_id, pgid).await
                    }
                    _ => Ok(()),
                };
                // 回填失败要原样上报——它比命令本身的失败更严重：台账与真实进程脱节了。
                // 进程由 `guard` 在析构时杀掉，心跳由 `heartbeat` 在析构时收掉。
                backfilled?;
                let collect =
                    self.spawn_streaming_collector(guard.take(), command_id, collected.clone());
                tokio::time::timeout(Duration::from_secs(req.timeout_sec), collect).await
            }
            Err(e) => Ok(Err(e)),
        };
        heartbeat.abort();

        let duration_ms = started.elapsed().as_millis() as u64;
        let (exit_code, stdout, stderr, timed_out) = match output {
            Ok(Ok(status)) => {
                let out = collected.lock().unwrap().clone();
                (status.code(), out.stdout, out.stderr, false)
            }
            Ok(Err(e)) => (None, String::new(), format!("命令启动失败：{e}"), false),
            Err(_) => {
                // 超时：杀掉整个进程组，已收到的输出仍保留（推流过的部分不丢）
                let out = collected.lock().unwrap().clone();
                if let Some(pgid) = child_pgid {
                    let _ = self.killer.kill_process_group(pgid);
                }
                (
                    None,
                    out.stdout,
                    format!("命令超时（{}s）", req.timeout_sec),
                    true,
                )
            }
        };

        // 决策 118：输出脱敏在**回填 messages 之前**执行
        let out = CommandOutput {
            command_id,
            exit_code,
            stdout: sanitize_text(&stdout),
            stderr: sanitize_text(&stderr),
            timed_out,
            duration_ms,
        };

        // 收尾那一行由调用点填（三处各自的 `stdout_path` 语义不同，见 `run` 的说明）
        let (record, extra) = finish(&out)?;

        if let Some(rec) = &self.recorder {
            if let Some(id) = command_id {
                rec.record_finish(id, record).await?;
            }
            // 命令结束刷新心跳（决策 100）
            rec.touch_heartbeat(req.owner.run_id).await?;
        }

        if out.timed_out {
            // 超时由节点级重试处理；这里把失败形态交给 agent loop，并杀掉整个进程组
            //（pgid 已在启动时捕获并回填 node_runs，决策 66 / 票 17）
            if let Some(pgid) = child_pgid {
                self.killer.kill_process_group(pgid)?;
            }
        }

        Ok((out, extra))
    }

    fn spawn(
        &self,
        req: &CommandRequest<'_>,
        command: &str,
        env: &ChildEnv,
    ) -> std::io::Result<tokio::process::Child> {
        match &req.spawn {
            SpawnForm::Shell => crate::process::spawn_in_own_process_group(command, req.cwd, env),
            SpawnForm::Argv { program, args } => {
                crate::process::spawn_argv_in_own_process_group(program, args, req.cwd, env)
            }
        }
    }

    /// 逐行读子进程输出、全量缓冲并**按行推流**（票 14 / 决策 100 / §12.4.4）。
    ///
    /// 返回子进程退出状态；stdout / stderr 的完整内容写进 `collected`。
    /// 推流只作用于观测面：超长行截断、超高频停止推流，均**不影响**缓冲的完整输出
    /// （`kanban_node_commands` 的落库口径不变）。无 `sse` 或无 `command_id` 时
    /// 退化为纯缓冲（不阻塞、不漏内容，短命令与无订阅者场景不退化）。
    async fn spawn_streaming_collector(
        &self,
        mut child: tokio::process::Child,
        command_id: Option<i64>,
        collected: Arc<std::sync::Mutex<CollectedOutput>>,
    ) -> std::io::Result<std::process::ExitStatus> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdout_pipe = child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>);
        let stderr_pipe = child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>);
        let sse = self.sse.clone();

        let pump = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
                    is_stderr: bool,
                    collected: Arc<std::sync::Mutex<CollectedOutput>>,
                    sse: Option<CommandSse>| async move {
            let Some(pipe) = pipe else { return };
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // 1) 完整缓冲（未脱敏原样；脱敏在回填前统一做，保持既有顺序）
                {
                    let mut c = collected.lock().unwrap();
                    if is_stderr {
                        c.stderr.push_str(&line);
                        c.stderr.push('\n');
                    } else {
                        c.stdout.push_str(&line);
                        c.stdout.push('\n');
                    }
                    // 2) 推流（受节流约束）
                    if let (Some(sse), Some(cmd_id)) = (sse.as_ref(), command_id) {
                        if c.streamed_lines < STREAM_MAX_LINES {
                            c.streamed_lines += 1;
                            drop(c);
                            let chunk = if line.chars().count() > STREAM_MAX_LINE_CHARS {
                                let head: String =
                                    line.chars().take(STREAM_MAX_LINE_CHARS).collect();
                                format!("{head}…[本行超长已截断]")
                            } else {
                                line.clone()
                            };
                            // 推流内容同样脱敏（§12.4.4：四条路径一致）
                            let chunk = sanitize_text(&chunk);
                            sse.sink.emit(crate::sse::SseEvent::CommandOutput {
                                task_id: sse.task_id.clone(),
                                branch: sse.branch.clone(),
                                command_id: cmd_id,
                                chunk,
                            });
                            continue;
                        }
                        // 超过行数上限：只推一次「已停止推流」标注
                        if c.streamed_lines == STREAM_MAX_LINES {
                            c.streamed_lines += 1;
                            drop(c);
                            sse.sink.emit(crate::sse::SseEvent::CommandOutput {
                                task_id: sse.task_id.clone(),
                                branch: sse.branch.clone(),
                                command_id: cmd_id,
                                chunk: format!(
                                    "…[输出超过 {STREAM_MAX_LINES} 行，已停止推流；完整内容以命令记录为准]"
                                ),
                            });
                        }
                    }
                }
            }
        };

        tokio::join!(
            pump(stdout_pipe, false, collected.clone(), sse.clone()),
            pump(stderr_pipe, true, collected, sse)
        );
        child.wait().await
    }

    /// 周期心跳任务：命令结束（含超时）时由调用方 abort（决策 100）。
    fn spawn_command_heartbeat(&self, run_id: Option<i64>) -> Option<tokio::task::JoinHandle<()>> {
        let recorder = self.recorder.clone()?;
        let interval = self.heartbeat_interval;
        Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // interval 的首次 tick 立即完成，跳过（起止心跳已覆盖）
            loop {
                tick.tick().await;
                if recorder.touch_heartbeat(run_id).await.is_err() {
                    break;
                }
            }
        }))
    }
}

/// 逐行收集的命令输出（票 14）：完整缓冲 + 推流计数。
#[derive(Debug, Clone, Default)]
struct CollectedOutput {
    stdout: String,
    stderr: String,
    /// 已推送的行数（用于节流；两条流合计）。
    streamed_lines: usize,
}

/// 运行期心跳任务：**作用域结束就 abort**（决策 100）。
///
/// 做成 guard 而不是每处手写 `abort()`：收口里有**提前返回**的路径（回填进程组失败），
/// 手写必然漏那一处——而漏了的心跳任务会一直按周期刷 `last_activity_at`，
/// 表现为「这条命令早就结束了，它的 run 却永远不空闲」。
struct Heartbeat(Option<tokio::task::JoinHandle<()>>);

impl Heartbeat {
    fn abort(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.abort();
    }
}

/// 「回填进程组失败时别把已 spawn 的 Child 丢掉」。
///
/// 一条既有的漏杀路（票 01 点名）：`set_process_group` 写库失败时那里是 `?`，
/// 于是函数在流式采集器建起来之前返回，`Child` 被丢掉——而全仓没有 `kill_on_drop`，
/// 那个进程就没人回收了。这里显式兜住：没被 [`Self::take`] 走的那一份在析构时杀掉。
struct ChildGuard(Option<tokio::process::Child>);

impl ChildGuard {
    fn new(child: tokio::process::Child) -> Self {
        ChildGuard(Some(child))
    }

    /// 交出 `Child`（此后不再由本 guard 负责）。
    fn take(&mut self) -> tokio::process::Child {
        self.0.take().expect("Child 只交出去一次")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // `start_kill` 只发信号、不等待：析构里不能 await。
            let _ = child.start_kill();
        }
    }
}
