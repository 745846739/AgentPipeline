//! 工具执行层（§10.2，决策 45 / 104 / 118 / 110）。
//!
//! **工具层全部真实执行**（决策 148）：write_file 真写、run_command 真跑、FileToolPolicy
//! 真拦、输出脱敏真过、L2 卸载真落盘、命令真记 `kanban_node_commands`。FakeAgent 只替换
//! LLM 响应流，因此集成测试顺带覆盖整个工具子系统。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use super::client::ToolCall;
use super::context::{
    count_tokens, needs_offload, offload_replacement, trim_list_dir, trim_read_file,
    trim_run_command,
};
use super::file_policy::FileToolPolicy;
use super::sanitize::sanitize_text;
use crate::config::{effective_run_command_timeout, Settings};
use crate::home::Home;
use crate::process::ProcessKiller;
use crate::storage::tasks::TaskFilter;
use crate::storage::Store;
use crate::types::{CommandSource, Node, Stage};
use crate::{Error, Result};

use super::egress::NetworkPolicy;

/// 值班长读回执时的消息裁剪（决策 182⑭）：只留最后 N 条，且总量压在字符上限内。
///
/// 两次裁剪的必要性不同：条数上限挡住「一次几百轮的会话」，字符上限挡住「一条消息
/// 本身就有几万字」（`run_command` 的完整输出会整段进 messages）。只做前者会在一条
/// 巨长的消息上失效，只做后者会让一万条短消息挤满预算。
fn trim_conversation_messages(messages: &serde_json::Value) -> Vec<serde_json::Value> {
    use crate::pipeline::foreman::{
        FOREMAN_CONVERSATION_MAX_CHARS, FOREMAN_CONVERSATION_MAX_MESSAGES,
    };

    let Some(all) = messages.as_array() else {
        return Vec::new();
    };
    let tail = if all.len() > FOREMAN_CONVERSATION_MAX_MESSAGES {
        &all[all.len() - FOREMAN_CONVERSATION_MAX_MESSAGES..]
    } else {
        &all[..]
    };
    let mut kept: Vec<serde_json::Value> = Vec::new();
    let mut used = 0usize;
    for msg in tail.iter().rev() {
        let rendered = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let cost = rendered.chars().count();
        if !kept.is_empty() && used + cost > FOREMAN_CONVERSATION_MAX_CHARS {
            break;
        }
        used += cost;
        kept.push(msg.clone());
    }
    kept.reverse();
    kept
}

/// 命令日志记录的启动信息（§12.4.4）。
#[derive(Debug, Clone)]
pub struct CommandStart {
    /// 归属任务。值班长的命令给 `None`，改为挂 [`Self::session_id`]（决策 204④）。
    /// 空串由存储层归一成 `None`——空串不是归属，它是「没有」的伪装。
    pub task_id: Option<String>,
    /// 归属会话（值班长的命令）。流水线命令为 `None`。
    pub session_id: Option<String>,
    pub run_id: Option<i64>,
    pub stage: Stage,
    pub node: Node,
    pub source: CommandSource,
    pub command: String,
    pub cwd: String,
}

/// 命令日志记录的收尾信息。
#[derive(Debug, Clone, Default)]
pub struct CommandFinish {
    pub exit_code: Option<i32>,
    pub stdout_path: Option<String>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub duration_ms: u64,
}

/// 命令记录器接缝（存储层实现；测试用记录器）。
pub trait CommandRecorder: Send + Sync + 'static {
    fn record_start(&self, start: CommandStart) -> BoxFuture<'static, Result<i64>>;
    fn record_finish(
        &self,
        command_id: i64,
        finish: CommandFinish,
    ) -> BoxFuture<'static, Result<()>>;
    /// 刷新所属 run 的 `last_activity_at`（决策 100：长命令不得被空闲超时误杀）。
    fn touch_heartbeat(&self, run_id: Option<i64>) -> BoxFuture<'static, Result<()>>;

    /// 回填 run 的真实进程组 id（决策 66 / 票 17）：scheduler 超时时据此杀整个进程组。
    /// 默认空实现——不关心 pgid 的记录器（含测试替身）无需改。
    fn set_process_group(&self, _run_id: i64, _pgid: i32) -> BoxFuture<'static, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// 子代理调用的入参（决策 172③，票 08）。
///
/// 只带**每次调用不同**的东西：子任务文本。任务 / 分支 / attempt / 父 run / 只读工具集
/// 这些「同一次节点执行内固定」的信息由 [`SubAgentRunner`] 的实现持有——它是按
/// attempt 构造的，天然知道自己在哪个节点、哪一次执行、属于哪个父 run 里。
pub struct SubAgentRequest {
    /// 父代理给出的子任务描述。
    pub task: String,
}

/// 子代理执行接缝（决策 172③，票 08）。
///
/// 与 [`CommandRecorder`] 同一种做法：工具层只声明「我需要一个能跑子代理的东西」，
/// 真正的 agent 循环住在 pipeline 层（它才持有 LLM 接缝）。**工具层不认识 LLM**，
/// 所以子代理不能从工具层自己长出来。
///
/// 没有注入实现时 `spawn_sub_agent` 不可用——这正是「默认关闭」的落点：能力由阶段
/// 声明与 executor 接线共同决定，而不是由工具层假装支持。
pub trait SubAgentRunner: Send + Sync + 'static {
    fn run(&self, request: SubAgentRequest) -> BoxFuture<'static, Result<String>>;
}

/// 工具调用上下文。
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub task_id: String,
    /// 归属会话（值班长的命令挂会话，决策 204④）。流水线节点为 `None`——
    /// 它不是「另一个称呼的 task_id」，是另一条归属。
    pub session_id: Option<String>,
    pub stage: Stage,
    pub node: Node,
    pub worktree_path: PathBuf,
    pub task_dir: PathBuf,
    pub run_id: Option<i64>,
    /// 命令来源：agent 的 `run_command` 为 [`CommandSource::Agent`]。
    pub command_source: CommandSource,
    /// `run_command` 的默认真实 cwd（卫生默认值，**不是安全边界**）。
    pub default_cwd: Option<PathBuf>,
}

impl ToolCallContext {
    /// 该阶段产出写入哪个根（§6 / §8）。
    ///
    /// 设计 / 评审文档写任务目录；代码写 worktree；test 阶段的集成代码写 worktree，
    /// 但 `test-report.md` 是任务目录的固定产出（pipeline-spec §6：test 写集成代码到
    /// worktree、test-report.md到任务目录）。
    pub fn write_root_for(&self, relative: &str) -> &Path {
        match self.stage {
            Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign | Stage::Review => {
                &self.task_dir
            }
            Stage::Test
                if Path::new(relative).file_name()
                    == Some(std::ffi::OsStr::new("test-report.md")) =>
            {
                &self.task_dir
            }
            _ => &self.worktree_path,
        }
    }

    /// 读路径解析顺序：先 worktree，再任务目录。
    pub fn read_candidates(&self, relative: &str) -> Vec<PathBuf> {
        vec![
            self.worktree_path.join(relative),
            self.task_dir.join(relative),
        ]
    }
}

/// 工具执行结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    /// 回填进 messages 的文本（**已脱敏**，决策 118）。
    pub content: String,
    /// `submit_metadata` 提交的结构化元数据。
    pub metadata: Option<serde_json::Value>,
}

impl ToolOutcome {
    fn ok(content: impl Into<String>) -> Self {
        ToolOutcome {
            content: content.into(),
            metadata: None,
        }
    }
}

/// 工具执行器。
pub struct ToolExecutor {
    home: Home,
    policy: FileToolPolicy,
    settings: Settings,
    recorder: Option<Arc<dyn CommandRecorder>>,
    killer: Arc<dyn ProcessKiller>,
    /// `run_command` 运行期间的心跳周期（决策 100）；默认 5s，测试可调短。
    command_heartbeat_interval: Duration,
    /// 流式输出去向（决策 100 / §12.4.4，票 14）：`run_command` 按行推送命令输出。
    /// `None` = 不推流（纯单测 / 无订阅者场景），行为与既有缓冲一致。
    sse: Option<CommandSse>,
    /// 子代理执行器（决策 172③，票 08）。`None` = `spawn_sub_agent` 不可用
    /// ——默认关闭即由此表达，而非让工具层假装支持。
    sub_agent: Option<Arc<dyn SubAgentRunner>>,
    /// **强制**的工具白名单（决策 172③，票 08）。`None` = 无限制（父节点的常态）。
    ///
    /// 这是子代理只读边界的真正落点。只限制「广告出去的 tool 定义」是不够的：
    /// [`Self::execute`] 按 `call.name` 路由，模型完全可以无视 tool 定义直接发一个
    /// `run_command`，那样它就真被跑掉了。故边界必须落在**执行点**。
    ///
    /// 类型是 `Vec<&'static str>` 而不是 `&'static [&'static str]`：值班长的白名单要能
    /// **由工具清单生成**（票 01 的「广告集与白名单同源」），而 const 数组做不到按层过滤。
    /// 名字本身仍是 `'static`，故「白名单是硬编码的，不是运行时可配的」这条性质不变。
    allow: Option<Vec<&'static str>>,
    /// `run_command` 的出口策略（决策 179，票 12）。
    ///
    /// 构造时从 [`Settings`] 取一次（见 [`Self::new`]），执行点不再读配置——策略与「这次执行
    /// 用的哪份设置」不会错位。默认姿态保守：空清单 + 不放行全部，只放行回环。
    egress: NetworkPolicy,
    /// 台账读句柄（决策 182⑭，票 02）。`None` = `read_task` / `read_conversation` 不可用。
    ///
    /// 这两个工具**只面向值班长**：它的输入是人可以随便打的任意文本，故它的能力必须来自
    /// 一个显式注入的只读句柄，而不是继承流水线节点那套（含文件与命令）的上下文
    /// ——「不注入即不可用」让「它到底能碰什么」在构造点就看得见。
    ledger: Option<Store>,
}

/// 命令输出推流的上下文（票 14）：SSE 去向 + 事件里要带的任务/分支。
#[derive(Clone)]
pub struct CommandSse {
    pub sink: Arc<dyn crate::sse::SseSink>,
    pub task_id: String,
    pub branch: String,
}

/// 单行推流上限（票 14 的节流策略之一）：超长行截断并标注，避免一行撑爆事件。
pub const STREAM_MAX_LINE_CHARS: usize = 4_000;
/// 单条命令最多推送的行数（票 14 的节流策略之二）：高频输出超过后停止推流并标注，
/// **完整输出仍全量缓冲**用于命令记录与回填——推流是观测面，不是数据来源。
pub const STREAM_MAX_LINES: usize = 2_000;

/// 逐行收集的命令输出（票 14）：完整缓冲 + 推流计数。
#[derive(Debug, Clone, Default)]
struct CollectedOutput {
    stdout: String,
    stderr: String,
    /// 已推送的行数（用于节流；两条流合计）。
    streamed_lines: usize,
}

/// 心跳默认周期：远小于 300s 空闲超时，600s 级测试命令也能存活（决策 100）。
pub const COMMAND_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

impl ToolExecutor {
    pub fn new(
        home: Home,
        policy: FileToolPolicy,
        settings: Settings,
        killer: Arc<dyn ProcessKiller>,
    ) -> Self {
        ToolExecutor {
            home,
            policy,
            egress: NetworkPolicy::from_settings(&settings),
            settings,
            recorder: None,
            killer,
            command_heartbeat_interval: COMMAND_HEARTBEAT_INTERVAL,
            sse: None,
            sub_agent: None,
            allow: None,
            ledger: None,
        }
    }

    pub fn with_recorder(mut self, recorder: Arc<dyn CommandRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// 注入子代理执行器（决策 172③，票 08）。不注入即 `spawn_sub_agent` 不可用。
    pub fn with_sub_agent(mut self, runner: Arc<dyn SubAgentRunner>) -> Self {
        self.sub_agent = Some(runner);
        self
    }

    /// 把工具集**收窄**为给定的白名单（决策 172③，票 08）。
    ///
    /// 与「只少给几个 tool 定义」不同：越界的调用在 [`Self::execute`] 处被拒，模型
    /// 就算硬发也执行不了。子代理的只读边界靠它成立。
    pub fn with_allowed_tools(mut self, allow: Vec<&'static str>) -> Self {
        self.allow = Some(allow);
        self
    }

    /// 注入台账读句柄（决策 182⑭，票 02）：使 `read_task` / `read_conversation` 可用。
    ///
    /// 与 [`Self::with_sub_agent`] 同一种做法——能力由接线决定，不注入即不可用。
    /// 注入的是 [`Store`] 本身而不是一层新 trait：台账是既有的存储实现，
    /// 为它再造一个可替换接缝只会多一个「测试里跑的不是真 SQL」的口子，
    /// 而这两个工具要验的恰恰是「读得到真台账」。
    pub fn with_ledger(mut self, store: Store) -> Self {
        self.ledger = Some(store);
        self
    }

    /// 注入命令输出流式去向（决策 100 / 票 14）：长命令按行推 `command_output`。
    pub fn with_sse(mut self, sse: CommandSse) -> Self {
        self.sse = Some(sse);
        self
    }

    /// 测试用：调短 `run_command` 的周期心跳。
    pub fn with_command_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.command_heartbeat_interval = interval;
        self
    }

    /// 覆盖 `run_command` 的出口策略（决策 179，票 12）。
    ///
    /// 生产路径由 [`Self::new`] 从 [`Settings`] 直接取，没有这一步；它存在是为了让用例
    /// 能单独钉住策略（尤其是「同一份设置下放行 / 拒绝两种走向」），而不必绕道配置。
    pub fn with_egress(mut self, egress: NetworkPolicy) -> Self {
        self.egress = egress;
        self
    }

    pub fn egress(&self) -> &NetworkPolicy {
        &self.egress
    }

    pub fn policy(&self) -> &FileToolPolicy {
        &self.policy
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 执行一次工具调用。
    ///
    /// 白名单（[`Self::with_allowed_tools`]）在**这里**生效——先于任何分发。只靠
    /// tool 定义约束是纸糊的：模型可以无视定义直接发 `run_command`。
    pub async fn execute(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        if let Some(allow) = &self.allow {
            if !allow.contains(&call.name.as_str()) {
                return Err(Error::Validation(format!(
                    "工具 {} 不在本次调用的允许集内（只读子代理仅允许：{}）",
                    call.name,
                    allow.join(" / ")
                )));
            }
        }
        let outcome = match call.name.as_str() {
            "write_file" => self.write_file(call, ctx).await?,
            "edit_file" => self.edit_file(call, ctx).await?,
            "read_file" => self.read_file(call, ctx).await?,
            "delete_file" => self.delete_file(call, ctx).await?,
            "list_dir" => self.list_dir(call, ctx).await?,
            "run_command" => self.run_command(call, ctx).await?,
            "submit_metadata" => self.submit_metadata(call)?,
            "Skill" => self.skill(call)?,
            "spawn_sub_agent" => self.spawn_sub_agent(call, ctx).await?,
            // 台账只读工具（决策 182⑭，票 02）。它们在白名单里的位置与其余工具相同：
            // 越权调用在函数开头的白名单检查处就被拒，这里不再重复判定「谁可以调」。
            "read_task" => self.read_task(call).await?,
            "read_conversation" => self.read_conversation(call).await?,
            // A 层环境读数（决策 188 / 207，票 01）：全部只读，全部走后端既有口径。
            "read_board" => self.read_board().await?,
            "read_metrics" => self.read_metrics().await?,
            "read_projects" => self.read_projects().await?,
            "read_stage_configs" => self.read_stage_configs().await?,
            "read_skills" => self.read_skills().await?,
            "read_providers" => self.read_providers().await?,
            other => return Err(Error::Validation(format!("未知工具：{other}"))),
        };
        self.apply_l2_offload(call, ctx, outcome)
    }

    /// L2 大结果卸载**覆盖全部工具**（决策 110 / 票 04）：任何工具结果超过
    /// `offload_threshold_tokens` 一律落盘、context 只留预览 + 路径。
    ///
    /// `run_command` 在自身路径里已按 stdout/stderr 语义卸载（保留退出码与失败行），
    /// 此处跳过避免二次卸载；`submit_metadata` 是极小 JSON，无需处理。
    ///
    /// 值班长的两个台账工具同样跳过：卸载要写 `home.context_dir(&ctx.task_id)`，
    /// 而值班长**没有 task_id**（空串会落到上下文根目录，污染下一个真实任务的文件）。
    /// 它们的结果在工具内部已按字符上限截断，不会无界增长。
    fn apply_l2_offload(
        &self,
        call: &ToolCall,
        ctx: &ToolCallContext,
        outcome: ToolOutcome,
    ) -> Result<ToolOutcome> {
        if matches!(
            call.name.as_str(),
            "run_command" | "submit_metadata" | "read_task" | "read_conversation"
        ) {
            return Ok(outcome);
        }
        if !needs_offload(&outcome.content, &self.settings) {
            return Ok(outcome);
        }
        let dir = self.home.context_dir(&ctx.task_id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.txt", ulid::Ulid::new()));
        std::fs::write(&path, &outcome.content)?;
        let tokens = count_tokens(&outcome.content);
        let preview = head_tail(&outcome.content, 30, 30);
        Ok(ToolOutcome {
            content: offload_replacement(&call.name, &path.display().to_string(), tokens, &preview),
            metadata: outcome.metadata,
        })
    }

    fn args(call: &ToolCall) -> Result<serde_json::Value> {
        serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("工具 {} 参数解析失败：{e}", call.name)))
    }

    async fn write_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("write_file 缺少 path".into()))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("write_file 缺少 content".into()))?;

        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 先清后写保证幂等（G9）
        std::fs::write(&resolved, content)?;
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{}\",\"bytes\":{}}}",
            rel,
            content.len()
        )))
    }

    async fn edit_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
        let new_text = args.get("new_text").and_then(|v| v.as_str()).unwrap_or("");

        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        let original = std::fs::read_to_string(&resolved)?;
        if !original.contains(old_text) {
            return Err(Error::Validation(format!(
                "edit_file 未找到待替换文本（{rel}）"
            )));
        }
        // 幂等：只替换一次
        let updated = original.replacen(old_text, new_text, 1);
        std::fs::write(&resolved, updated)?;
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{rel}\"}}"
        )))
    }

    /// 派生一个**只读**子代理（决策 172③，票 08）。
    ///
    /// 父代理给出子任务描述，子代理在独立 context 里跑完并把**摘要**带回父对话——
    /// 「读 20 个文件」的原文因此不会进父上下文。工具集由 pipeline 层的运行器固定为
    /// `read_file` / `list_dir`，**不继承阶段声明的工具**（阶段配置无法给子代理扩权）。
    ///
    /// 未注入运行器时返回**错误文本而非 `Err`**：与 `Skill` 工具同一姿态（票 06）——
    /// `Err` 会被算作工具失败并累计 `tool_retry_max`，模型因此打挂整个节点；返回文本
    /// 让模型自行改道。
    async fn spawn_sub_agent(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let Some(runner) = &self.sub_agent else {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 在当前阶段未启用。请直接完成该子任务，\
                 或在阶段配置的 tools_json 里声明 spawn_sub_agent。",
            ));
        };
        let args = Self::args(call)?;
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        if task.trim().is_empty() {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 需要 {task} 参数（子任务描述）。请补充后重试。",
            ));
        }
        // 父 run 由运行器自己持有（它按 attempt 构造）。这里仍要求 ctx 带 run_id：
        // 缺它说明调用不在节点执行的上下文里，那种情况不该派生（会落无父的孤儿 run）。
        if ctx.run_id.is_none() {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 需要所属 run 上下文（当前调用没有 run_id），无法派生。",
            ));
        }
        let summary = runner
            .run(SubAgentRequest {
                task: task.to_string(),
            })
            .await?;
        Ok(ToolOutcome::ok(summary))
    }

    async fn read_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let mut found: Option<PathBuf> = None;
        for candidate in ctx.read_candidates(rel) {
            if candidate.exists() {
                let resolved = self.policy.check_read(&candidate)?;
                found = Some(resolved);
                break;
            }
        }
        let path = found.ok_or_else(|| Error::Validation(format!("文件不存在：{rel}")))?;
        let content = std::fs::read_to_string(&path)?;
        let sliced = if offset > 0 || limit.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let end = limit
                .map(|l| (offset + l).min(lines.len()))
                .unwrap_or(lines.len());
            lines[offset.min(lines.len())..end].join("\n")
        } else {
            content
        };
        // L1 裁剪：默认头部 200 行 + 结构大纲
        Ok(ToolOutcome::ok(trim_read_file(&sliced, limit)))
    }

    async fn delete_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        // 不存在视为成功（幂等，G9）
        if resolved.exists() {
            std::fs::remove_file(&resolved)?;
        }
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{rel}\"}}"
        )))
    }

    async fn list_dir(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let root = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            ctx.worktree_path.join(rel)
        };
        let resolved = self.policy.check_read(&root)?;
        let mut entries = Vec::new();
        collect_entries(&resolved, recursive, &mut entries)?;
        entries.sort();
        Ok(ToolOutcome::ok(trim_list_dir(&entries)))
    }

    fn submit_metadata(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let value: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("submit_metadata 参数解析失败：{e}")))?;
        Ok(ToolOutcome {
            content: "{\"success\":true}".to_string(),
            metadata: Some(value),
        })
    }

    /// `Skill` 工具（决策 172③，票 06）：按名取技能正文，作为 **tool result** 进 `messages`。
    ///
    /// **不走 `Err` 通道**——未知技能名返回一段说明文本而非 `Error`。理由在 agent loop 的
    /// 分层里：`Err` 会被算作工具失败并累计到 `tool_retry_max`（决策 33），模型写错一个技能名
    /// 就可能把整个节点打挂；而票 06 明确要求这种情况**让模型自行纠正**。返回文本既进上下文
    /// 又不触发失败计数，模型下一轮换个名字即可。
    ///
    /// 读的是**技能根**（loader 侧），不经 [`FileToolPolicy`]——技能根与 `{home}/data/`
    /// （provider 密钥明文存储，决策 112）同父，放宽为 agent 可读等于交出密钥。
    fn skill(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("Skill 参数解析失败：{e}")))?;
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.trim().is_empty() {
            return Ok(ToolOutcome::ok(
                "Skill 工具需要 {name} 参数（技能名）。请用技能目录里列出的名字重试。",
            ));
        }
        match crate::agent::skills::load_body(&self.home.skills_dir(), name) {
            Ok(body) => Ok(ToolOutcome::ok(body)),
            Err(e) => Ok(ToolOutcome::ok(format!(
                "无法加载技能 {name}：{e}。\
                 请从技能目录里选一个名字重试；若该技能尚未安装，请先安装再调用。"
            ))),
        }
    }

    /// `read_task`（决策 182⑭，票 02）：读某个任务的台账详情。
    ///
    /// **不存在时不走 `Err` 通道**，与 [`Self::skill`] 同一理由：模型写错一个任务 id 是
    /// 最常见的失败，`Err` 会被算作工具失败并累计 `tool_retry_max`（决策 33），
    /// 一次笔误就能把整次回话打挂。返回一段说明文本既进上下文又不触发失败计数。
    async fn read_task(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "read_task 需要 {task_id} 参数。任务 id 是态势快照里方括号内那串。",
            ));
        }
        // 「任务不存在」在这里是**正常回答**而不是故障：模型可能记错一个 id。
        // 只吞 `Error::Task`（那是「查无此任务」），其余错误照常上升——把库故障
        // 也说成「没有这个任务」会让模型继续拿错 id 反复猜。
        let task = match store.get_task(&task_id).await {
            Ok(t) => t,
            Err(Error::Task(_)) => {
                return Ok(ToolOutcome::ok(format!(
                    "台账里没有任务 {task_id}。请用态势快照里列出的 id 重试。"
                )))
            }
            Err(e) => return Err(e),
        };
        // allowed_actions 由后端权威下发（决策 101）——这里把它**原样**交出去，
        // 不做筛选也不做解释。值班长能替值班经理描述「可按下哪些键」，
        // 但它自己按不动（写动作仍需人来发）。
        let cursors = store.load_live_cursors(&task_id).await?;
        let actions = task
            .pending_reason
            .as_ref()
            .map(|r| crate::actions::allowed_actions(r, None))
            .unwrap_or_default();
        let value = serde_json::json!({
            "task_id": task.id,
            "title": task.title,
            "description": task.description,
            "status": task.status.as_str(),
            "current_stage": task.current_stage.as_str(),
            "current_node": task.current_node.as_str(),
            "pending_reason": task.pending_reason,
            "allowed_actions": actions,
            "cursors": cursors.iter().map(|c| serde_json::json!({
                "branch": c.branch,
                "stage": c.stage,
                "node": c.node,
                "status": c.status.as_str(),
            })).collect::<Vec<_>>(),
            "total_tokens": task.total_tokens,
            "total_calls": task.total_calls,
            "stalled": task.stalled,
            "updated_at": task.updated_at.to_rfc3339(),
        });
        // `description` 与 `allowed_actions` 都可能很长，而这个结果**不走** L2 卸载
        // （见 `apply_l2_offload`），所以上限必须在这里落。
        Ok(ToolOutcome::ok(
            crate::pipeline::foreman::truncate_tool_result(&serde_json::to_string_pretty(&value)?),
        ))
    }

    /// 台账读句柄。六个 A 层读数与两个台账工具共用它（票 01）。
    ///
    /// 不注入即不可用——值班长的能力必须来自一个显式注入的只读句柄，
    /// 而不是继承流水线节点那套（含文件与命令）的上下文。
    fn ledger_or_err(&self) -> Result<&Store> {
        self.ledger
            .as_ref()
            .ok_or_else(|| Error::Validation("这个工具不可用：本次调用没有注入台账读句柄".into()))
    }

    /// 一个读数 → 交出去的文本。
    ///
    /// 统一收口三件事：不美化（`to_string_pretty` 便于模型读）、**按字符上限截断**
    /// （这些结果不走 L2 卸载——卸载要写 `home.context_dir(&ctx.task_id)`，而值班长没有
    /// task_id）、失败不 panic。
    fn readout(value: serde_json::Value) -> ToolOutcome {
        let text = match serde_json::to_string_pretty(&value) {
            Ok(t) => t,
            Err(e) => format!("读数序列化失败：{e}"),
        };
        ToolOutcome::ok(crate::pipeline::foreman::truncate_tool_result(&text))
    }

    /// `read_board`（票 01）：整块看板——每个任务的状态与当前工位 + 按状态的分组计数。
    ///
    /// 口径**与看板端点同源**（同一张任务表、同一组状态字符串），不新造一套读数。
    /// 与态势快照的分工：快照只装「需要有人管的」（待拍板 / 在跑 / 失败），
    /// 这里是全量——问「一共多少活」时要看得到已完成与排队。
    async fn read_board(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let tasks = store
            .list_tasks(&TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?;
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for t in &tasks {
            *counts.entry(t.status.as_str()).or_default() += 1;
        }
        // 按状态分组列出（组内按 id，保证同一份数据两次调用给出同一个顺序）
        let mut by_status: std::collections::BTreeMap<&str, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for t in &tasks {
            by_status
                .entry(t.status.as_str())
                .or_default()
                .push(serde_json::json!({
                    "task_id": t.id,
                    "title": t.title,
                    "stage": t.current_stage.as_str(),
                    "node": t.current_node.as_str(),
                    "updated_at": t.updated_at.to_rfc3339(),
                }));
        }
        Ok(Self::readout(serde_json::json!({
            "counts": counts,
            "tasks_by_status": by_status,
        })))
    }

    /// `read_metrics`（票 01）：全局指标，**复用 `metrics::*` 纯函数口径**（决策 130② / 137）。
    ///
    /// 不在这里另写一套 SQL 聚合：那是指标页与端点的契约，两处各写一份必然漂移。
    async fn read_metrics(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let tasks = store
            .list_tasks(&TaskFilter {
                include_archived: true,
                ..Default::default()
            })
            .await?;
        let statuses: Vec<crate::types::TaskStatus> = tasks.iter().map(|t| t.status).collect();
        let runs = store.all_runs().await?;
        let aggregation = store.stage_aggregation().await?;
        Ok(Self::readout(serde_json::json!({
            "tasks": tasks.len(),
            "success_rate": crate::metrics::success_rate(&statuses),
            "validate_first_pass_rate": crate::metrics::validate_first_pass_rate(&runs),
            "total_tokens": crate::metrics::total_tokens(&runs),
            "total_calls": crate::metrics::total_calls(&runs),
            // 阶段聚合的既有形状是 `(stage, 平均时长, 重试率, 总次数)`——`GET /metrics`
            // 与指标页都用这一份口径，故字段名逐字对齐（`avg_duration_ms` / `retry_rate` /
            // `total_runs`）。**不在这里给它改名换姓**：改过名的读数会让值班长把「平均时长」
            // 当成 token 数报给值班经理（第一次实现里就是错的）。
            "stage_aggregation": aggregation.iter().map(|(stage, avg_duration, retry_rate, total)| {
                serde_json::json!({
                    "stage": stage,
                    "avg_duration_ms": avg_duration,
                    "retry_rate": retry_rate,
                    "total_runs": total,
                })
            }).collect::<Vec<_>>(),
        })))
    }

    /// `read_projects`（票 01）：已接入的项目清单。
    async fn read_projects(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let projects = store.list_projects().await?;
        Ok(Self::readout(serde_json::json!({
            "projects": projects.iter().map(|p| serde_json::json!({
                "project_id": p.id,
                "name": p.name,
                "local_path": p.local_path,
                "default_branch": p.default_branch,
                "language": p.language,
                "test_framework": p.test_framework,
                "lint_command": p.lint_command,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_stage_configs`（票 01）：各阶段的配置。
    ///
    /// 原样交出去（含 `persona_path` / 技能声明）：这些是配置读数，不是秘密；
    /// 而选一个字段藏起来，模型就会开始猜「为什么这个工位是这样」。
    async fn read_stage_configs(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let configs = store.list_stage_configs().await?;
        Ok(Self::readout(serde_json::json!({
            "stage_configs": configs.iter().map(|c| serde_json::json!({
                "stage": c.stage,
                "provider_id": c.provider_id,
                "temperature": c.temperature,
                "max_tokens": c.max_tokens,
                "persona_path": c.persona_path,
                "skills_json": c.skills_json,
                "idle_timeout_sec": c.idle_timeout_sec,
                "max_duration_sec": c.max_duration_sec,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_skills`（票 01）：技能根下可用的技能 + 被谁引用。
    ///
    /// 与 `GET /skills` 同源（同一个 `discover`），不另走一条发现逻辑。
    async fn read_skills(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let configs = store.list_stage_configs().await?;
        let skills = crate::agent::skills::discover(&self.home.skills_dir());
        Ok(Self::readout(serde_json::json!({
            "skills": skills.iter().map(|s| serde_json::json!({
                "name": s.name,
                "description": s.frontmatter.description,
                "path": s.path.display().to_string(),
                "declared_in": crate::config::declared_skill_where(&configs, &s.name),
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_providers`（票 01）：provider 清单，**密钥只回显掩码**（决策 112）。
    ///
    /// 库里存的是明文，故这里走**既有的**掩码读法 `list_providers_masked()`
    /// （provider 端点读的也是它），不是在这里另写一个正则。
    async fn read_providers(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        // `list_providers_masked` 是**既有的**那一份掩码读法（provider 端点也走它）：
        // 库里存的是明文密钥（决策 112），而「这个 provider 配没配密钥」值班长该知道，
        // 密钥本身对它没有用处。
        let providers = store.list_providers_masked().await?;
        Ok(Self::readout(serde_json::json!({
            "providers": providers.iter().map(|p| serde_json::json!({
                "provider_id": p.id,
                "vendor": p.vendor,
                "model": p.model,
                "enabled": p.enabled,
                "context_window": p.context_window,
                "base_url": p.base_url,
                // 已是掩码（上面的 `list_providers_masked`），不是原文。
                "api_key": p.api_key,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_conversation`（决策 182⑭，票 02）：读某次节点运行的会话回执。
    ///
    /// `run_id` 缺省取该任务**最近一次**会话——人是按「那个货箱卡哪儿了」提问的，
    /// 不是按运行 id；让模型非要先查 run 列表才能问，等于把台账的内部编号变成使用门槛。
    async fn read_conversation(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "read_conversation 需要 {task_id} 参数（可再加 {run_id}）。",
            ));
        }
        let requested_run = args.get("run_id").and_then(|v| v.as_i64());
        let run_id = match requested_run {
            Some(id) => id,
            None => match store.list_conversations(&task_id, false).await?.last() {
                Some(last) => last.run_id,
                None => {
                    return Ok(ToolOutcome::ok(format!(
                        "任务 {task_id} 还没有任何节点会话——它可能还没跑到调 LLM 的节点。"
                    )))
                }
            },
        };
        let conversation = match store.get_conversation(&task_id, run_id).await? {
            Some(c) => c,
            None => {
                return Ok(ToolOutcome::ok(format!(
                    "任务 {task_id} 的 {run_id} 号运行没有会话回执（可能是纯代码节点，\
                     或该运行已被清理）。用 read_task 看它当前停在哪个工位。"
                )))
            }
        };
        let messages = trim_conversation_messages(&conversation.messages_json);
        let value = serde_json::json!({
            "task_id": task_id,
            "run_id": run_id,
            "stage": conversation.stage.as_str(),
            "node": conversation.node.as_str(),
            "attempt": conversation.attempt,
            "agent_type": conversation.agent_type,
            "prompt_tokens": conversation.prompt_tokens,
            "completion_tokens": conversation.completion_tokens,
            "messages": messages,
        });
        // 消息已按条数与字符双重截断，但包上 stage / agent_type 等字段后仍可能略超上限；
        // 这里再兜一次（与 `read_task` 同一理由：它不走 L2 卸载）。
        Ok(ToolOutcome::ok(
            crate::pipeline::foreman::truncate_tool_result(&serde_json::to_string_pretty(&value)?),
        ))
    }

    async fn run_command(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("run_command 缺少 command".into()))?
            .to_string();
        let explicit_timeout = args.get("timeout_sec").and_then(|v| v.as_u64());
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.default_cwd.clone())
            .unwrap_or_else(|| ctx.worktree_path.clone());

        // 命令脱敏后落库（§12.4.4）
        let sanitized = super::sanitize::sanitize_command_line(&command);

        // 出口策略（决策 179，票 12）在**启动进程之前**判定：被拒的命令根本不执行。
        // 拒绝也要落 `kanban_node_commands`（与放行的命令同表）——审计面必须看得见
        // 「有过一次被拒的出口尝试」，否则策略只是一次静默失败。
        if let Err(denied) = self.egress.check(&command) {
            let id = self.record_command_start(ctx, &sanitized, &cwd).await?;
            if let (Some(rec), Some(id)) = (self.recorder.as_ref(), id) {
                rec.record_finish(
                    id,
                    CommandFinish {
                        exit_code: Some(crate::agent::egress::EGRESS_DENIED_EXIT_CODE),
                        stderr_preview: Some(denied.to_string()),
                        ..Default::default()
                    },
                )
                .await?;
            }
            return Err(denied);
        }

        let command_id = self.record_command_start(ctx, &sanitized, &cwd).await?;

        // 命令开始即刷新心跳（决策 100）
        if let Some(rec) = &self.recorder {
            rec.touch_heartbeat(ctx.run_id).await?;
        }

        let timeout_sec =
            effective_run_command_timeout(&self.settings, ctx.stage, explicit_timeout);
        // 决策 100：运行期间周期心跳——600s 级命令不被 300s 空闲超时误杀
        let heartbeat = self.spawn_command_heartbeat(ctx.run_id);
        let started = Instant::now();
        // 独立进程组启动（票 17 / 决策 66）：捕获真实 pgid 回填 node_runs，
        // 超时回调终止器杀整个进程组（此前 kill(0) 是 no-op）。
        let mut child_pgid: Option<i32> = None;
        // 逐行读 + 按行推流（票 14 / 决策 100 / §12.4.4）：输出经管道进入后台收集任务，
        // 完整内容全量缓冲用于落库与回填（推流是观测面，不改变 kanban_node_commands 口径）。
        let collected = std::sync::Arc::new(std::sync::Mutex::new(CollectedOutput::default()));
        let output = match crate::process::spawn_in_own_process_group(&command, &cwd) {
            Ok(child) => {
                child_pgid = child.id().map(|id| id as i32);
                if let (Some(rec), Some(run_id), Some(pgid)) =
                    (self.recorder.as_ref(), ctx.run_id, child_pgid)
                {
                    rec.set_process_group(run_id, pgid).await?;
                }
                // 流式收集：stdout / stderr 各起一条读行任务，边读边推 event
                let collect = self.spawn_streaming_collector(child, command_id, collected.clone());
                tokio::time::timeout(std::time::Duration::from_secs(timeout_sec), collect).await
            }
            Err(e) => Ok(Err(e)),
        };
        if let Some(task) = &heartbeat {
            task.abort();
        }

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
                    format!("命令超时（{timeout_sec}s）"),
                    true,
                )
            }
        };

        // 决策 118：输出脱敏在**回填 messages 之前**执行
        let stdout = sanitize_text(&stdout);
        let stderr = sanitize_text(&stderr);

        // L1 裁剪 + L2 卸载（唯一阈值，决策 110）
        let (in_context, offload_path) = self.prepare_output(&ctx.task_id, &stdout, &stderr)?;

        if let Some(rec) = &self.recorder {
            if let Some(id) = command_id {
                rec.record_finish(
                    id,
                    CommandFinish {
                        exit_code,
                        stdout_path: offload_path.clone(),
                        stdout_preview: Some(head_tail(&stdout, 50, 100)),
                        stderr_preview: Some(head_tail(&stderr, 50, 100)),
                        duration_ms,
                    },
                )
                .await?;
            }
            // 命令结束刷新心跳（决策 100）
            rec.touch_heartbeat(ctx.run_id).await?;
        }

        if timed_out {
            // 超时由节点级重试处理；这里把失败形态交给 agent loop，并杀掉整个进程组
            //（pgid 已在启动时捕获并回填 node_runs，决策 66 / 票 17）
            if let Some(pgid) = child_pgid {
                self.killer.kill_process_group(pgid)?;
            }
        }

        Ok(ToolOutcome::ok(in_context))
    }

    /// 落一条命令日志的「开始」并返回 id（未接记录器时 `None`）。
    ///
    /// 被拒的出口与正常执行**走同一个入口**（决策 179，票 12）：审计面必须看得见每一次
    /// 尝试，包括被拒的那些——否则策略在日志里完全不可见，只剩模型侧的一次报错。
    async fn record_command_start(
        &self,
        ctx: &ToolCallContext,
        sanitized: &str,
        cwd: &Path,
    ) -> Result<Option<i64>> {
        let Some(rec) = &self.recorder else {
            return Ok(None);
        };
        Ok(Some(
            rec.record_start(CommandStart {
                task_id: Some(ctx.task_id.clone()),
                session_id: ctx.session_id.clone(),
                run_id: ctx.run_id,
                stage: ctx.stage,
                node: ctx.node,
                source: ctx.command_source,
                command: sanitized.to_string(),
                cwd: cwd.display().to_string(),
            })
            .await?,
        ))
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
        collected: std::sync::Arc<std::sync::Mutex<CollectedOutput>>,
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
                    collected: std::sync::Arc<std::sync::Mutex<CollectedOutput>>,
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
        let interval = self.command_heartbeat_interval;
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

    /// 输出裁剪 + 卸载，返回（进 context 的文本，卸载路径）。
    fn prepare_output(
        &self,
        task_id: &str,
        stdout: &str,
        stderr: &str,
    ) -> Result<(String, Option<String>)> {
        let combined = if stderr.is_empty() {
            stdout.to_string()
        } else {
            format!("{stdout}\n[stderr]\n{stderr}")
        };

        if !needs_offload(&combined, &self.settings) {
            return Ok((trim_run_command(&combined), None));
        }

        let dir = self.home.context_dir(task_id);
        std::fs::create_dir_all(&dir)?;
        let name = format!("{}.txt", ulid::Ulid::new());
        let path = dir.join(name);
        std::fs::write(&path, &combined)?;
        let preview = head_tail(&combined, 30, 30);
        let tokens = count_tokens(&combined);
        Ok((
            offload_replacement("run_command", &path.display().to_string(), tokens, &preview),
            Some(path.display().to_string()),
        ))
    }
}

fn collect_entries(root: &Path, recursive: bool, out: &mut Vec<String>) -> Result<()> {
    // 只读目录，策略已通过 check_read 校验
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type()?.is_dir();
        out.push(if is_dir { format!("{name}/") } else { name });
        if recursive && is_dir {
            collect_entries(&path, recursive, out)?;
        }
    }
    Ok(())
}

/// 首尾摘录（L2 预览 / 命令 preview 用）。
pub fn head_tail(text: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head + tail {
        return text.to_string();
    }
    let mut out: Vec<String> = lines[..head].iter().map(|l| l.to_string()).collect();
    out.push(format!("... 省略 {} 行 ...", lines.len() - head - tail));
    out.extend(lines[lines.len() - tail..].iter().map(|l| l.to_string()));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 记录器替身：记录调用，不落库。
    #[derive(Default)]
    struct RecordingRecorder {
        starts: Mutex<Vec<CommandStart>>,
        finishes: Mutex<Vec<(i64, CommandFinish)>>,
        heartbeats: Mutex<u32>,
    }

    impl CommandRecorder for RecordingRecorder {
        fn record_start(&self, start: CommandStart) -> BoxFuture<'static, Result<i64>> {
            // 每次调用自增 id
            let starts = self.starts.lock().unwrap();
            let id = starts.len() as i64 + 1;
            drop(starts);
            self.starts.lock().unwrap().push(start);
            Box::pin(async move { Ok(id) })
        }

        fn record_finish(
            &self,
            command_id: i64,
            finish: CommandFinish,
        ) -> BoxFuture<'static, Result<()>> {
            self.finishes.lock().unwrap().push((command_id, finish));
            Box::pin(async move { Ok(()) })
        }

        fn touch_heartbeat(&self, _run_id: Option<i64>) -> BoxFuture<'static, Result<()>> {
            *self.heartbeats.lock().unwrap() += 1;
            Box::pin(async move { Ok(()) })
        }
    }

    struct NoKiller;
    impl ProcessKiller for NoKiller {
        fn kill_process_group(&self, _pgid: i32) -> Result<()> {
            Ok(())
        }
    }

    struct Setup {
        _tmp: tempfile::TempDir,
        home: Home,
        executor: ToolExecutor,
        ctx: ToolCallContext,
        worktree: PathBuf,
        task_dir: PathBuf,
    }

    fn setup(stage: Stage) -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();
        let worktree = home.worktree_path("t1");
        let task_dir = home.task_dir("t1");
        home.ensure_task_dirs("t1").unwrap();

        let policy = FileToolPolicy::new(vec![worktree.clone(), task_dir.clone()]);
        let executor = ToolExecutor::new(
            home.clone(),
            policy,
            Settings::default(),
            Arc::new(NoKiller),
        );
        let ctx = ToolCallContext {
            task_id: "t1".into(),
            session_id: None,
            stage,
            node: Node::Execute,
            worktree_path: worktree.clone(),
            task_dir: task_dir.clone(),
            run_id: Some(1),
            command_source: CommandSource::Agent,
            default_cwd: Some(worktree.clone()),
        };
        Setup {
            _tmp: tmp,
            home,
            executor,
            ctx,
            worktree,
            task_dir,
        }
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.to_string(),
        }
    }

    #[tokio::test]
    async fn write_file_writes_design_doc_to_task_dir() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "design.md", "content": "# 设计"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("\"success\":true"));
        assert_eq!(
            std::fs::read_to_string(s.task_dir.join("design.md")).unwrap(),
            "# 设计"
        );
        assert!(
            !s.worktree.join("design.md").exists(),
            "设计文档不得写进 worktree"
        );
    }

    #[tokio::test]
    async fn write_file_writes_code_to_worktree() {
        let s = setup(Stage::Develop);
        s.executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "src/main.rs", "content": "fn main(){}"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(s.worktree.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn write_file_is_idempotent() {
        let s = setup(Stage::ArchitectDesign);
        for _ in 0..2 {
            s.executor
                .execute(
                    &call(
                        "write_file",
                        serde_json::json!({"path": "design.md", "content": "v1"}),
                    ),
                    &s.ctx,
                )
                .await
                .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(s.task_dir.join("design.md")).unwrap(),
            "v1"
        );
    }

    #[tokio::test]
    async fn file_policy_blocks_dotenv_write() {
        let s = setup(Stage::Develop);
        let err = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": ".env", "content": "K=v"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn file_policy_blocks_path_outside_roots() {
        let s = setup(Stage::Develop);
        let err = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "/etc/passwd", "content": "x"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn read_file_falls_back_from_worktree_to_task_dir() {
        let s = setup(Stage::Develop);
        std::fs::write(s.task_dir.join("design.md"), "line1\nline2").unwrap();
        let out = s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "design.md"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "line1\nline2");
    }

    #[tokio::test]
    async fn read_file_missing_is_an_error() {
        let s = setup(Stage::Develop);
        assert!(s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "nope.md"})),
                &s.ctx
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn read_file_supports_offset_limit() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "l0\nl1\nl2\nl3").unwrap();
        let out = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "a.txt", "offset": 1, "limit": 2}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "l1\nl2");
    }

    #[tokio::test]
    async fn edit_file_replaces_once_and_errors_when_missing() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "x x x").unwrap();
        s.executor
            .execute(
                &call(
                    "edit_file",
                    serde_json::json!({"path": "a.txt", "old_text": "x", "new_text": "y"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(s.worktree.join("a.txt")).unwrap(),
            "y x x"
        );

        assert!(s
            .executor
            .execute(
                &call(
                    "edit_file",
                    serde_json::json!({"path": "a.txt", "old_text": "zzz", "new_text": "q"}),
                ),
                &s.ctx,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn delete_file_is_idempotent() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "x").unwrap();
        for _ in 0..2 {
            s.executor
                .execute(
                    &call("delete_file", serde_json::json!({"path": "a.txt"})),
                    &s.ctx,
                )
                .await
                .unwrap();
        }
        assert!(!s.worktree.join("a.txt").exists());
    }

    #[tokio::test]
    async fn list_dir_caps_and_marks_directories() {
        let s = setup(Stage::Develop);
        std::fs::create_dir_all(s.worktree.join("src")).unwrap();
        for i in 0..5 {
            std::fs::write(s.worktree.join(format!("f{i}.rs")), "").unwrap();
        }
        let out = s
            .executor
            .execute(&call("list_dir", serde_json::json!({"path": "."})), &s.ctx)
            .await
            .unwrap();
        assert!(out.content.contains("src/"));
        assert!(out.content.contains("f0.rs"));
    }

    #[tokio::test]
    async fn run_command_executes_for_real_and_records() {
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());

        let out = executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo hello"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("hello"), "实际输出：{}", out.content);

        let starts = recorder.starts.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].source, CommandSource::Agent);
        assert_eq!(starts[0].command, "echo hello");
        drop(starts);

        let finishes = recorder.finishes.lock().unwrap();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1.exit_code, Some(0));
        assert!(finishes[0].1.duration_ms < 60_000);
        drop(finishes);

        // 命令开始与结束都刷新心跳（决策 100）
        assert_eq!(*recorder.heartbeats.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn run_command_touches_heartbeat_periodically_during_long_commands() {
        // 决策 100：长命令（如 600s 测试）靠运行期周期心跳躲过 300s 空闲超时
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone())
        .with_command_heartbeat_interval(std::time::Duration::from_millis(50));
        executor
            .execute(
                &call("run_command", serde_json::json!({"command": "sleep 0.3"})),
                &s.ctx,
            )
            .await
            .unwrap();
        // 起止各一次 + 运行期间若干次
        assert!(*recorder.heartbeats.lock().unwrap() >= 4, "周期心跳未生效");
    }

    #[tokio::test]
    async fn run_command_output_is_sanitized_before_backfill() {
        // 决策 118：agent 看到的即脱敏后文本
        let s = setup(Stage::Develop);
        let secret = "sk-abcdefghijklmnop12345678";
        let out = s
            .executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": format!("echo {secret}")}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains(secret), "输出未脱敏：{}", out.content);
        assert!(out.content.contains("***"));
    }

    #[tokio::test]
    async fn run_command_commands_are_sanitized_in_the_log() {
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());
        executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "deploy --token sk-abcdefghijklmnop12345678"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        let starts = recorder.starts.lock().unwrap();
        assert!(starts[0].command.contains("--token ***"));
        assert!(!starts[0].command.contains("sk-abcdefghijklmnop"));
    }

    #[tokio::test]
    async fn env_var_values_are_sanitized_in_output_and_command_log() {
        // 票 15 / §12.4.4：环境变量值脱敏在**输出回填**与**命令记录**两条路径一致。
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());
        // 命令自身含敏感环境变量赋值；输出回显同样的赋值
        let out = executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "export API_TOKEN=abc123value; echo \"API_TOKEN=abc123value PATH=$PATH\""}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        // 工具结果（回填 agent messages）不含明文
        assert!(
            !out.content.contains("abc123value"),
            "工具结果未脱敏：{}",
            out.content
        );
        assert!(out.content.contains("API_TOKEN=***"));
        // 命令记录同样不含明文
        let starts = recorder.starts.lock().unwrap();
        assert!(!starts[0].command.contains("abc123value"));
        assert!(starts[0].command.contains("API_TOKEN=***"));
    }

    #[tokio::test]
    async fn benign_env_var_values_survive_command_output() {
        // 票 15：无害变量（PATH / 纯数字 / 非敏感名）不被误伤，输出原样保留。
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "echo \"PATH=/usr/bin FOO=secret RETRIES=3\""}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("PATH=/usr/bin"));
        assert!(out.content.contains("FOO=secret"));
        assert!(out.content.contains("RETRIES=3"));
    }

    #[tokio::test]
    async fn large_output_is_offloaded_to_context_dir() {
        let s = setup(Stage::Develop);
        let settings = Settings {
            offload_threshold_tokens: 10, // 降低阈值便于测试
            ..Default::default()
        };
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            settings,
            Arc::new(NoKiller),
        );
        let out = executor
            .execute(
                &call("run_command", serde_json::json!({"command": "seq 1 5000"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("已卸载"), "应走 L2：{}", out.content);
        let offloaded = std::fs::read_dir(s.home.context_dir("t1")).unwrap().count();
        assert_eq!(offloaded, 1, "卸载文件应真实落盘（决策 148）");
    }

    #[tokio::test]
    async fn small_output_is_not_offloaded() {
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo tiny"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains("已卸载"));
        assert_eq!(
            std::fs::read_dir(s.home.context_dir("t1")).unwrap().count(),
            0
        );
    }

    #[tokio::test]
    async fn submit_metadata_returns_typed_payload() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(
                &call(
                    "submit_metadata",
                    serde_json::json!({"readiness": true, "affected_files": []}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.metadata.unwrap()["readiness"], true);
    }

    // ── 票 06（决策 172③）：`Skill` 工具 ──

    /// 在技能根（`{home}/skills`）下写一个用户技能。
    fn write_home_skill(s: &Setup, name: &str, content: &str) {
        let dir = s.home.skills_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
    }

    #[tokio::test]
    async fn skill_tool_returns_skill_body() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "grill", "拷问协议：把设计树走完");
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "grill"})), &s.ctx)
            .await
            .unwrap();
        assert_eq!(out.content, "拷问协议：把设计树走完");
        assert!(out.metadata.is_none(), "Skill 不是 submit_metadata");
    }

    /// 票 07：`Skill` 工具返回的正文**含一级展开的兄弟文件**。
    ///
    /// 上游技能的 `[tests.md](tests.md)` 是双向死指针（文件工具读不到技能目录），
    /// 展开走加载器；二级引用不递归。
    #[tokio::test]
    async fn skill_tool_inlines_siblings_one_level() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "tdd", "主文档\n\n[tests.md](tests.md)");
        let dir = s.home.skills_dir().join("tdd");
        std::fs::write(
            dir.join("tests.md"),
            "兄弟：一个用例一件事\n[deep.md](deep.md)",
        )
        .unwrap();
        std::fs::write(dir.join("deep.md"), "二级内容不该出现").unwrap();

        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "tdd"})), &s.ctx)
            .await
            .unwrap();
        assert!(out.content.contains("主文档"), "{}", out.content);
        assert!(
            out.content.contains("一个用例一件事"),
            "一级兄弟文件应内联：{}",
            out.content
        );
        assert!(
            !out.content.contains("二级内容不该出现"),
            "二级引用不得展开：{}",
            out.content
        );
    }

    /// 缺失的兄弟文件 → 工具返回错误文本（走文本通道，不触发 `tool_retry_max`）。
    #[tokio::test]
    async fn skill_tool_missing_sibling_returns_text() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "broken", "[gone.md](gone.md)");
        let out = s
            .executor
            .execute(
                &call("Skill", serde_json::json!({"name": "broken"})),
                &s.ctx,
            )
            .await
            .expect("技能包残缺不得走 Err 通道");
        assert!(out.content.contains("gone.md"), "{}", out.content);
        assert!(out.content.contains("broken"), "{}", out.content);
    }

    /// 未知技能名**不是工具失败**——返回说明文本让模型自行纠正（票 06）。
    ///
    /// 若走 `Err`，agent loop 会把它算进 `tool_retry_max`（决策 33），模型写错一个名字
    /// 就可能把整个节点打挂。
    #[tokio::test]
    async fn skill_tool_unknown_name_returns_text_not_error() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "nope"})), &s.ctx)
            .await
            .expect("未知技能名不得走 Err 通道");
        assert!(out.content.contains("nope"), "{}", out.content);
        assert!(out.content.contains("无法加载"), "{}", out.content);
    }

    /// 缺 `name` 参数同样返回可读文本，而不是 `Err`。
    #[tokio::test]
    async fn skill_tool_missing_name_returns_text() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({})), &s.ctx)
            .await
            .expect("缺参不得走 Err 通道");
        assert!(out.content.contains("name"), "{}", out.content);
    }

    /// 技能根**不经** `FileToolPolicy`——`read_file` 读技能根会被拒，`Skill` 工具能读。
    ///
    /// 这条同时钉住决策 172 的安全边界：技能根与 `{home}/data/`（provider 密钥明文存储，
    /// 决策 112）同父，**不得**被放宽为 agent 可读；兄弟文件走加载器展开而非放宽文件策略。
    #[tokio::test]
    async fn skill_tool_reads_skill_root_that_file_policy_refuses() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "secret-ish", "技能正文");
        let skill_path = s.home.skills_dir().join("secret-ish").join("SKILL.md");

        // 文件工具被策略挡住（技能根不在 worktree / 任务目录内）
        assert!(
            s.executor
                .execute(
                    &call(
                        "read_file",
                        serde_json::json!({"path": skill_path.display().to_string()})
                    ),
                    &s.ctx,
                )
                .await
                .is_err(),
            "read_file 不得读技能根"
        );

        // 而 Skill 工具（loader 侧）能取到正文
        let out = s
            .executor
            .execute(
                &call("Skill", serde_json::json!({"name": "secret-ish"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "技能正文");
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let s = setup(Stage::Develop);
        assert!(s
            .executor
            .execute(
                &call("definitely_not_a_tool", serde_json::json!({})),
                &s.ctx
            )
            .await
            .is_err());
    }

    /// 票 08：未注入子代理运行器时，`spawn_sub_agent` **不是**未知工具——
    /// 它返回说明文本（走文本通道，不烧 `tool_retry_max`），与 `Skill` 的未知技能
    /// 名同一姿态。这样「工具存在但本轮未启用」与「工具名写错」是两件事。
    #[tokio::test]
    async fn spawn_sub_agent_without_runner_returns_text_not_error() {
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call(
                    "spawn_sub_agent",
                    serde_json::json!({"task": "找出所有调用点"}),
                ),
                &s.ctx,
            )
            .await
            .expect("未启用不得走 Err 通道");
        assert!(out.content.contains("未启用"), "{}", out.content);
    }

    /// 票 08：子代理运行器已注入但缺 `task` → 提示补参（仍是文本通道）。
    #[tokio::test]
    async fn spawn_sub_agent_requires_task_argument() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let out = executor
            .execute(&call("spawn_sub_agent", serde_json::json!({})), &s.ctx)
            .await
            .expect("缺参不得走 Err 通道");
        assert!(out.content.contains("task"), "{}", out.content);
    }

    /// 票 08：缺父 run（无 run_id）时拒绝派生——否则会落一行无父的孤儿 run。
    #[tokio::test]
    async fn spawn_sub_agent_without_run_id_is_refused() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let mut ctx = s.ctx.clone();
        ctx.run_id = None;
        let out = executor
            .execute(
                &call("spawn_sub_agent", serde_json::json!({"task": "检索"})),
                &ctx,
            )
            .await
            .expect("缺 run 上下文不得走 Err 通道");
        assert!(out.content.contains("run_id"), "{}", out.content);
    }

    /// 票 08：注入运行器后，子代理返回的摘要原样进 tool_result。
    #[tokio::test]
    async fn spawn_sub_agent_returns_runner_summary() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let out = executor
            .execute(
                &call("spawn_sub_agent", serde_json::json!({"task": "检索"})),
                &s.ctx,
            )
            .await
            .expect("正常路径不得报错");
        assert_eq!(out.content, "子代理摘要");
    }

    /// 测试替身：不调 LLM，直接回固定摘要。
    struct NoSubAgent;

    impl SubAgentRunner for NoSubAgent {
        fn run(
            &self,
            _request: SubAgentRequest,
        ) -> futures::future::BoxFuture<'static, Result<String>> {
            Box::pin(async { Ok("子代理摘要".to_string()) })
        }
    }

    #[test]
    fn head_tail_keeps_both_ends() {
        let text = (0..500)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = head_tail(&text, 2, 2);
        assert!(out.contains("l0"));
        assert!(out.contains("l499"));
        assert!(out.contains("省略 496 行"));
        assert!(!out.contains("l100"));
    }

    #[tokio::test]
    async fn l2_offload_covers_non_command_tools() {
        // 票 04 / 决策 110：L2 卸载覆盖**全部工具**，不再只对 run_command 生效。
        // read_file 显式要求大 limit 时 L1 不裁剪（用户点名要这么多行），
        // 结果超 offload_threshold_tokens → 落盘 + 只留预览。
        let s = setup(Stage::Develop);
        let long = (0..20_000)
            .map(|i| format!("line {i} of a very long file"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(s.worktree.join("big.rs"), &long).unwrap();

        let out = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "big.rs", "limit": 20_000}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(
            out.content.contains("已卸载"),
            "read_file 超阈值结果应走 L2 卸载：{}",
            &out.content[..out.content.len().min(200)]
        );
        // 卸载文件真实落盘且含完整内容
        let ctx_dir = s.home.context_dir("t1");
        let files: Vec<_> = std::fs::read_dir(&ctx_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!files.is_empty(), "L2 卸载文件应落盘");
        assert!(
            files.iter().any(|f| std::fs::read_to_string(f.path())
                .map(|c| c.contains("line 19999"))
                .unwrap_or(false)),
            "卸载文件应含完整内容"
        );
    }

    #[tokio::test]
    async fn l2_offload_skips_small_results() {
        // 未超阈值的小结果原样返回（L2 不误伤）
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("small.rs"), "fn main() {}\n").unwrap();
        let out = s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "small.rs"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains("已卸载"));
        assert!(out.content.contains("fn main"));
    }

    /// 简单 SSE 录制器（只关心 command_output，验证推流路径）。
    #[derive(Default, Clone)]
    struct SseRecorder {
        chunks: Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl crate::sse::SseSink for SseRecorder {
        fn emit(&self, event: crate::sse::SseEvent) {
            if let crate::sse::SseEvent::CommandOutput { chunk, .. } = event {
                self.chunks.lock().unwrap().push(chunk);
            }
        }
    }

    #[tokio::test]
    async fn run_command_streams_output_lines_before_completion() {
        // 票 14 / 决策 100：命令执行期间按行推送 command_output，
        // 而不是等进程结束一次性缓冲。命令先输出再睡，推流应早于结束发生。
        let s = setup(Stage::Develop);
        let sse = SseRecorder::default();
        // command_id 由 recorder 分配——没有 recorder 就没有 command_id，也就没有推流
        // （这正是「无订阅者 / 无命令记录时不推流」的退化路径）。
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder)
        .with_sse(crate::agent::tools::CommandSse {
            sink: Arc::new(sse.clone()),
            task_id: "t1".into(),
            branch: "main".into(),
        });
        // 输出 → 睡 0.4s → 再输出：推流分两次，逐行到达
        let out = executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "echo first; sleep 0.4; echo second"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        // 完整输出仍全量回填（推流不改变回填口径）
        assert!(out.content.contains("first") && out.content.contains("second"));
        let chunks = sse.chunks.lock().unwrap().clone();
        assert!(
            chunks.iter().any(|c| c.contains("first")),
            "应推送首行：{chunks:?}"
        );
        assert!(
            chunks.iter().any(|c| c.contains("second")),
            "应推送后续行：{chunks:?}"
        );
    }

    #[tokio::test]
    async fn command_streaming_sanitizes_chunks() {
        // §12.4.4 / 票 14：推流路径同样脱敏，不出现「先推明文后脱敏」的窗口。
        let s = setup(Stage::Develop);
        let sse = SseRecorder::default();
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(Arc::new(RecordingRecorder::default()))
        .with_sse(crate::agent::tools::CommandSse {
            sink: Arc::new(sse.clone()),
            task_id: "t1".into(),
            branch: "main".into(),
        });
        executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "export API_TOKEN=abc123secret; echo done"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        let chunks = sse.chunks.lock().unwrap().clone();
        assert!(
            !chunks.iter().any(|c| c.contains("abc123secret")),
            "推流内容应已脱敏：{chunks:?}"
        );
    }

    #[tokio::test]
    async fn command_without_sse_still_buffers_fully() {
        // 无订阅者场景不退化（票 14）：不推流但完整输出照常回填。
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo hello"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("hello"));
    }
}
