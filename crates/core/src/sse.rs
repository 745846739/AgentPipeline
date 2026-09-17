//! SSE 事件（决策 76 / 84 / 123）。
//!
//! `/tasks/{id}/stream` 是**唯一**事件流，事件按 `type` 区分；所有事件体带 `branch`
//! 字段用于并行分支消歧（决策 84）。SSE 只是渲染通道——状态同步不依赖它，
//! 因此 cooldown / quiet_hours 只作用于前端 toast（决策 130 ③）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Node, Stage};

/// 事件类型标记（序列化进 `type` 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SseEventType {
    NodeStarted,
    NodeFinished,
    /// §12.7 的 `stage_changed`（阶段切换）。
    StageChanged,
    CursorChanged,
    Pending,
    PendingUpdated,
    CommandStarted,
    CommandOutput,
    CommandFinished,
    ConversationDelta,
    ToolEvent,
    Stalled,
    TaskCancelled,
    TaskDone,
    TaskFailed,
}

/// SSE 事件体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent {
    NodeStarted {
        task_id: String,
        branch: String,
        stage: Stage,
        node: Node,
        attempt: u32,
        run_id: i64,
    },
    NodeFinished {
        task_id: String,
        branch: String,
        stage: Stage,
        node: Node,
        attempt: u32,
        run_id: i64,
        status: String,
        duration_ms: u64,
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    StageChanged {
        task_id: String,
        branch: String,
        from_stage: Option<Stage>,
        from_node: Option<Node>,
        to_stage: Stage,
        to_node: Node,
        trigger: String,
        reason: Option<String>,
    },
    /// 决策 84：游标状态变化（新增事件类型）。
    CursorChanged {
        task_id: String,
        branch: String,
        cursor_id: String,
        status: String,
        stage: Stage,
        node: Node,
    },
    Pending {
        task_id: String,
        branch: String,
        cursor_id: String,
        reason: crate::types::PendingReason,
    },
    /// 决策 102：conflict_wait 复检后冲突对象变化。
    PendingUpdated {
        task_id: String,
        branch: String,
        cursor_id: String,
        context: crate::types::PendingContext,
    },
    CommandStarted {
        task_id: String,
        branch: String,
        command_id: i64,
        command: String,
        source: String,
    },
    CommandOutput {
        task_id: String,
        branch: String,
        command_id: i64,
        chunk: String,
    },
    CommandFinished {
        task_id: String,
        branch: String,
        command_id: i64,
        exit_code: Option<i32>,
        duration_ms: u64,
    },
    /// 决策 123：会话流式增量。`prompt_tokens` / `completion_tokens` 是**增量**
    /// （frontend-design 差距⑤：流式 token 计数的唯一来源）。
    ConversationDelta {
        task_id: String,
        branch: String,
        run_id: i64,
        agent_type: String,
        /// 归属会话（决策 204⑥）。流水线的增量为空串；`serde(default)` 让老客户端
        /// （不认识这个字段的一方）读到的事件照旧能解析——加字段是**加性**改动。
        #[serde(default)]
        session_id: String,
        role: String,
        text: String,
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    /// 决策 123：工具调用事件（参数只给摘要）。
    ToolEvent {
        task_id: String,
        branch: String,
        run_id: i64,
        tool: String,
        phase: ToolPhase,
        args_summary: String,
    },
    Stalled {
        task_id: String,
        branch: String,
        pending_hours: u64,
    },
    TaskCancelled {
        task_id: String,
        branch: String,
    },
    /// §12.7 终态事件按结果拆分：done / failed / cancelled（决策 84）。
    TaskDone {
        task_id: String,
        branch: String,
    },
    TaskFailed {
        task_id: String,
        branch: String,
    },
}

impl SseEvent {
    pub fn event_type(&self) -> SseEventType {
        match self {
            SseEvent::NodeStarted { .. } => SseEventType::NodeStarted,
            SseEvent::NodeFinished { .. } => SseEventType::NodeFinished,
            SseEvent::StageChanged { .. } => SseEventType::StageChanged,
            SseEvent::CursorChanged { .. } => SseEventType::CursorChanged,
            SseEvent::Pending { .. } => SseEventType::Pending,
            SseEvent::PendingUpdated { .. } => SseEventType::PendingUpdated,
            SseEvent::CommandStarted { .. } => SseEventType::CommandStarted,
            SseEvent::CommandOutput { .. } => SseEventType::CommandOutput,
            SseEvent::CommandFinished { .. } => SseEventType::CommandFinished,
            SseEvent::ConversationDelta { .. } => SseEventType::ConversationDelta,
            SseEvent::ToolEvent { .. } => SseEventType::ToolEvent,
            SseEvent::Stalled { .. } => SseEventType::Stalled,
            SseEvent::TaskCancelled { .. } => SseEventType::TaskCancelled,
            SseEvent::TaskDone { .. } => SseEventType::TaskDone,
            SseEvent::TaskFailed { .. } => SseEventType::TaskFailed,
        }
    }

    /// 事件所属分支（决策 84：所有事件体都带 `branch`）。
    pub fn branch(&self) -> &str {
        match self {
            SseEvent::NodeStarted { branch, .. }
            | SseEvent::NodeFinished { branch, .. }
            | SseEvent::StageChanged { branch, .. }
            | SseEvent::CursorChanged { branch, .. }
            | SseEvent::Pending { branch, .. }
            | SseEvent::PendingUpdated { branch, .. }
            | SseEvent::CommandStarted { branch, .. }
            | SseEvent::CommandOutput { branch, .. }
            | SseEvent::CommandFinished { branch, .. }
            | SseEvent::ConversationDelta { branch, .. }
            | SseEvent::ToolEvent { branch, .. }
            | SseEvent::Stalled { branch, .. }
            | SseEvent::TaskCancelled { branch, .. }
            | SseEvent::TaskDone { branch, .. }
            | SseEvent::TaskFailed { branch, .. } => branch,
        }
    }

    /// 是否属于**对讲台**（值班长）的对话流（决策 182⑥）。
    ///
    /// 判据只有身份串一处：值班长发的增量事件带 `agent_type = "foreman"` 与**空 task id**。
    /// 既有的 `/tasks/{id}/stream` 按 task id 精确匹配，空串永不等于真实任务 id，
    /// 故它对工头事件天然零干扰——这条过滤**不是**用来隔离的（隔离已经成立），
    /// 而是让新增的 `/foreman/stream` 说得清自己要哪一类事件。
    ///
    /// 只认会话增量：工具事件（`tool_event`）的载荷里没有 `agent_type`，
    /// 无法与流水线节点的工具调用区分。值班长的工具痕迹走**落库的 `traces_json`**
    /// 而不是实时事件（票 05），这也正是它不需要一个新事件变体的原因。
    pub fn is_foreman_event(&self) -> bool {
        matches!(
            self,
            SseEvent::ConversationDelta { agent_type, .. } if agent_type == crate::pipeline::foreman::FOREMAN_AGENT_TYPE
        )
    }

    pub fn task_id(&self) -> &str {
        match self {
            SseEvent::NodeStarted { task_id, .. }
            | SseEvent::NodeFinished { task_id, .. }
            | SseEvent::StageChanged { task_id, .. }
            | SseEvent::CursorChanged { task_id, .. }
            | SseEvent::Pending { task_id, .. }
            | SseEvent::PendingUpdated { task_id, .. }
            | SseEvent::CommandStarted { task_id, .. }
            | SseEvent::CommandOutput { task_id, .. }
            | SseEvent::CommandFinished { task_id, .. }
            | SseEvent::ConversationDelta { task_id, .. }
            | SseEvent::ToolEvent { task_id, .. }
            | SseEvent::Stalled { task_id, .. }
            | SseEvent::TaskCancelled { task_id, .. }
            | SseEvent::TaskDone { task_id, .. }
            | SseEvent::TaskFailed { task_id, .. } => task_id,
        }
    }

    /// 序列化为 SSE `data:` 载荷。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 工具事件阶段（决策 123）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPhase {
    Start,
    End,
    Error,
}

/// 进程内 SSE 广播（单机工具，v1 不落库）。
#[derive(Debug, Clone)]
pub struct SseBus {
    sender: tokio::sync::broadcast::Sender<SseEvent>,
}

impl Default for SseBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl SseBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        SseBus { sender }
    }

    pub fn publish(&self, event: SseEvent) {
        // 无订阅者时 send 返回 Err——不是错误（决策 130 ③：后端不吞事件语义，只是没人听）
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SseEvent> {
        self.sender.subscribe()
    }
}

/// 事件录制器接口：测试用它断言推送序列（testing.md §3.3）。
pub trait SseSink: Send + Sync + 'static {
    fn emit(&self, event: SseEvent);
}

impl SseSink for SseBus {
    fn emit(&self, event: SseEvent) {
        self.publish(event);
    }
}

/// 实时时间戳（事件不含时间戳字段时，前端按到达顺序渲染）。
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PendingContext, PendingKind, PendingReason};

    fn sample_events() -> Vec<SseEvent> {
        let reason =
            PendingReason::new(PendingKind::Timeout, Stage::Develop, Node::Execute, "超时");
        vec![
            SseEvent::NodeStarted {
                task_id: "t".into(),
                branch: "main".into(),
                stage: Stage::Develop,
                node: Node::Execute,
                attempt: 1,
                run_id: 1,
            },
            SseEvent::NodeFinished {
                task_id: "t".into(),
                branch: "main".into(),
                stage: Stage::Develop,
                node: Node::Execute,
                attempt: 1,
                run_id: 1,
                status: "success".into(),
                duration_ms: 100,
                prompt_tokens: 1,
                completion_tokens: 2,
            },
            SseEvent::StageChanged {
                task_id: "t".into(),
                branch: "develop-design".into(),
                from_stage: Some(Stage::DevelopDesign),
                from_node: Some(Node::ValidateOutput),
                to_stage: Stage::SyncCheck,
                to_node: Node::Execute,
                trigger: "normal".into(),
                reason: None,
            },
            SseEvent::CursorChanged {
                task_id: "t".into(),
                branch: "test-design".into(),
                cursor_id: "c".into(),
                status: "waiting_join".into(),
                stage: Stage::TestDesign,
                node: Node::ValidateOutput,
            },
            SseEvent::Pending {
                task_id: "t".into(),
                branch: "main".into(),
                cursor_id: "c".into(),
                reason,
            },
            SseEvent::PendingUpdated {
                task_id: "t".into(),
                branch: "main".into(),
                cursor_id: "c".into(),
                context: PendingContext::with_kind("conflict_wait"),
            },
            SseEvent::CommandStarted {
                task_id: "t".into(),
                branch: "main".into(),
                command_id: 1,
                command: "cargo test".into(),
                source: "system".into(),
            },
            SseEvent::CommandOutput {
                task_id: "t".into(),
                branch: "main".into(),
                command_id: 1,
                chunk: "running 1 test".into(),
            },
            SseEvent::CommandFinished {
                task_id: "t".into(),
                branch: "main".into(),
                command_id: 1,
                exit_code: Some(0),
                duration_ms: 12,
            },
            SseEvent::ConversationDelta {
                task_id: "t".into(),
                branch: "develop-design".into(),
                run_id: 7,
                agent_type: "pseudo:conflict_check".into(),
                session_id: String::new(),
                role: "assistant".into(),
                text: "正在比对".into(),
                prompt_tokens: 3,
                completion_tokens: 4,
            },
            SseEvent::ToolEvent {
                task_id: "t".into(),
                branch: "main".into(),
                run_id: 7,
                tool: "write_file".into(),
                phase: ToolPhase::End,
                args_summary: "design.md".into(),
            },
            SseEvent::Stalled {
                task_id: "t".into(),
                branch: "main".into(),
                pending_hours: 80,
            },
            SseEvent::TaskCancelled {
                task_id: "t".into(),
                branch: "main".into(),
            },
            SseEvent::TaskDone {
                task_id: "t".into(),
                branch: "main".into(),
            },
            SseEvent::TaskFailed {
                task_id: "t".into(),
                branch: "main".into(),
            },
        ]
    }

    #[test]
    fn every_event_carries_type_and_branch() {
        // 决策 84：所有事件体带 branch；决策 76：按 type 区分
        for ev in sample_events() {
            let json: serde_json::Value = serde_json::from_str(&ev.to_json()).unwrap();
            assert!(json.get("type").is_some(), "缺少 type：{json}");
            assert!(json.get("branch").is_some(), "缺少 branch：{json}");
            assert_eq!(json["type"], serde_json::to_value(ev.event_type()).unwrap());
            assert_eq!(json["branch"], ev.branch());
            assert_eq!(json["task_id"], ev.task_id());
        }
    }

    #[test]
    fn conversation_delta_fields_complete() {
        let ev = SseEvent::ConversationDelta {
            task_id: "t".into(),
            branch: "test-design".into(),
            run_id: 42,
            agent_type: "main".into(),
            session_id: "s1".into(),
            role: "assistant".into(),
            text: "hi".into(),
            prompt_tokens: 5,
            completion_tokens: 6,
        };
        let json: serde_json::Value = serde_json::from_str(&ev.to_json()).unwrap();
        for key in [
            "type",
            "task_id",
            "branch",
            "run_id",
            "agent_type",
            // 决策 204⑥：会话身份。工头增量靠它归到正确的班次，
            // 流水线的增量这里是空串。
            "session_id",
            "role",
            "text",
            "prompt_tokens",
            "completion_tokens",
        ] {
            assert!(json.get(key).is_some(), "conversation_delta 缺少 {key}");
        }
        assert_eq!(json["type"], "conversation_delta");
        assert_eq!(json["session_id"], "s1");
    }

    /// 加字段是**加性**改动：老客户端（不认识 `session_id` 的一方）读到的事件照旧能解析。
    #[test]
    fn conversation_delta_without_session_id_still_parses() {
        let raw = r#"{"type":"conversation_delta","task_id":"t","branch":"main","run_id":1,
                       "agent_type":"main","role":"assistant","text":"hi",
                       "prompt_tokens":0,"completion_tokens":0}"#;
        let ev: SseEvent = serde_json::from_str(raw).unwrap();
        match ev {
            SseEvent::ConversationDelta { session_id, .. } => assert_eq!(session_id, ""),
            other => panic!("解成了别的变体：{other:?}"),
        }
    }

    #[test]
    fn tool_event_fields_complete() {
        let ev = SseEvent::ToolEvent {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            tool: "read_file".into(),
            phase: ToolPhase::Start,
            args_summary: "src/a.rs".into(),
        };
        let json: serde_json::Value = serde_json::from_str(&ev.to_json()).unwrap();
        for key in [
            "type",
            "task_id",
            "branch",
            "run_id",
            "tool",
            "phase",
            "args_summary",
        ] {
            assert!(json.get(key).is_some(), "tool_event 缺少 {key}");
        }
        assert_eq!(json["phase"], "start");
    }

    #[tokio::test]
    async fn bus_publish_without_subscriber_is_not_an_error() {
        let bus = SseBus::new(8);
        bus.publish(SseEvent::TaskCancelled {
            task_id: "t".into(),
            branch: "main".into(),
        });
    }

    #[tokio::test]
    async fn bus_delivers_to_subscriber() {
        let bus = SseBus::new(8);
        let mut rx = bus.subscribe();
        bus.publish(SseEvent::Stalled {
            task_id: "t".into(),
            branch: "main".into(),
            pending_hours: 72,
        });
        let got = rx.recv().await.unwrap();
        assert_eq!(got.event_type(), SseEventType::Stalled);
    }
}
