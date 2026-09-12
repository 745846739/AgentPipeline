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
use crate::types::{CommandSource, Node, Stage};
use crate::{Error, Result};

/// 命令日志记录的启动信息（§12.4.4）。
#[derive(Debug, Clone)]
pub struct CommandStart {
    pub task_id: String,
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

/// 工具调用上下文。
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub task_id: String,
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
            settings,
            recorder: None,
            killer,
            command_heartbeat_interval: COMMAND_HEARTBEAT_INTERVAL,
        }
    }

    pub fn with_recorder(mut self, recorder: Arc<dyn CommandRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// 测试用：调短 `run_command` 的周期心跳。
    pub fn with_command_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.command_heartbeat_interval = interval;
        self
    }

    pub fn policy(&self) -> &FileToolPolicy {
        &self.policy
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 执行一次工具调用。
    pub async fn execute(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        match call.name.as_str() {
            "write_file" => self.write_file(call, ctx).await,
            "edit_file" => self.edit_file(call, ctx).await,
            "read_file" => self.read_file(call, ctx).await,
            "delete_file" => self.delete_file(call, ctx).await,
            "list_dir" => self.list_dir(call, ctx).await,
            "run_command" => self.run_command(call, ctx).await,
            "submit_metadata" => self.submit_metadata(call),
            other => Err(Error::Validation(format!("未知工具：{other}"))),
        }
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
        let command_id = match &self.recorder {
            Some(rec) => Some(
                rec.record_start(CommandStart {
                    task_id: ctx.task_id.clone(),
                    run_id: ctx.run_id,
                    stage: ctx.stage,
                    node: ctx.node,
                    source: ctx.command_source,
                    command: sanitized.clone(),
                    cwd: cwd.display().to_string(),
                })
                .await?,
            ),
            None => None,
        };

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
        let output = match crate::process::spawn_in_own_process_group(&command, &cwd) {
            Ok(child) => {
                child_pgid = child.id().map(|id| id as i32);
                if let (Some(rec), Some(run_id), Some(pgid)) =
                    (self.recorder.as_ref(), ctx.run_id, child_pgid)
                {
                    rec.set_process_group(run_id, pgid).await?;
                }
                tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_sec),
                    child.wait_with_output(),
                )
                .await
            }
            Err(e) => Ok(Err(e)),
        };
        if let Some(task) = &heartbeat {
            task.abort();
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        let (exit_code, stdout, stderr, timed_out) = match output {
            Ok(Ok(out)) => (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
                false,
            ),
            Ok(Err(e)) => (None, String::new(), format!("命令启动失败：{e}"), false),
            Err(_) => (
                None,
                String::new(),
                format!("命令超时（{timeout_sec}s）"),
                true,
            ),
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

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let s = setup(Stage::Develop);
        assert!(s
            .executor
            .execute(&call("spawn_sub_agent", serde_json::json!({})), &s.ctx)
            .await
            .is_err());
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
}
