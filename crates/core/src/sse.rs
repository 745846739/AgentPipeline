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
    /// 决策 207：值班长提议的到达 / 作废（只读事件）。
    ForemanProposal,
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
        /// 这一段增量是**回话**还是**思考**（决策 244）。
        ///
        /// 默认 `content`（`serde(default)`）：老客户端读到推理增量会照旧把它拼进回话里
        /// ——那是这条通道在今天之前的行为（它们根本没有推理增量），故默认值必须落在
        /// 「照旧」那一侧，而不是凭空给老客户端造一段它不认识的东西。
        #[serde(default)]
        channel: Channel,
        role: String,
        text: String,
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    /// 决策 207：值班长提议的生命周期事件（到达 / 执行 / 作废）。
    ///
    /// **单开一个只读事件，不混进 `conversation_delta`**：提议不是对话增量——它有自己的
    /// 生命周期（未决 → 执行 / 拒绝 / 过期）、自己的读取面（`GET /foreman/proposals`）、
    /// 以及自己的渲染形态（一颗确认钮）。混进增量里会让前端的时间线归约同时处理两种
    /// 时态（「已经说过的话」与「还没发生的事」），那正是决策 207 要避开的那种复杂。
    ///
    /// 载荷里的 `status` 是**结果**的词汇表那一套（`pending` / `executed` / `rejected` /
    /// `expired`），与落库的 `kanban_foreman_proposals.status` 同一个值。
    ForemanProposal {
        /// 两个恒空串：值班长的事件不带任务、也没有并行分支（照 `conversation_delta`
        /// 为它立的同一条口径）。留着它们是因为决策 84 的「所有事件体带 task_id / branch」
        /// 是一条整体不变式——为一个新事件开例外，读那条决策的人就得重新判断一次它还剩多少效力。
        task_id: String,
        branch: String,
        session_id: String,
        proposal_id: String,
        tool: String,
        status: String,
        summary: String,
        expires_at: String,
    },
    /// 决策 123：工具调用事件（参数只给摘要）。
    ///
    /// **身份串进载荷**（决策 244）：这个事件原先没有 `agent_type`，于是
    /// `is_foreman_event` 无法把它与流水线节点的工具调用区分，值班长的工具痕迹只能等
    /// 那一轮落库后从 `traces_json` 读回来——即「对话完结后才展示」。加上
    /// `agent_type` / `session_id` 之后，它走**既有的那条**过滤（与
    /// `conversation_delta` 同一个判据），值班长的工具调用因此在发生的那一刻就到达界面。
    /// 流水线节点照旧填 `"main"` / `"system"` / `"pseudo:*"` 与空会话，判据不变。
    ToolEvent {
        task_id: String,
        branch: String,
        run_id: i64,
        /// 发起这次调用的 agent 身份（`"foreman"` / `"main"` / `"system"` / `"pseudo:*"`）。
        #[serde(default)]
        agent_type: String,
        /// 归属会话（决策 204⑥）；流水线节点为空串，与 `conversation_delta` 同一条口径。
        #[serde(default)]
        session_id: String,
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
            SseEvent::ForemanProposal { .. } => SseEventType::ForemanProposal,
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
            | SseEvent::ForemanProposal { branch, .. }
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
    /// 只认会话增量与提议事件：工具事件（`tool_event`）的载荷里没有 `agent_type`，
    /// 无法与流水线节点的工具调用区分。值班长的工具痕迹走**落库的 `traces_json`**
    /// 而不是实时事件（票 05），这也正是它不需要一个新事件变体的原因。
    ///
    /// **2026-09-22 修订（决策 244）**：上一段的前半句不再成立——`tool_event` 现在**带**
    /// `agent_type` / `session_id`（见该变体的注释），于是值班长的工具调用与增量走同一个
    /// 判据。修订的动因是诉求：「把对讲台的 thinking 和工具调用都实时展示出来，不要像现在
    /// 这样在对话完结后展示」——痕迹落库那条路本身没问题（审计照旧靠它），有问题的是它
    /// **只在轮次结束后**才到得了界面。
    pub fn is_foreman_event(&self) -> bool {
        match self {
            SseEvent::ConversationDelta { agent_type, .. } => {
                agent_type == crate::pipeline::foreman::FOREMAN_AGENT_TYPE
            }
            SseEvent::ToolEvent { agent_type, .. } => {
                agent_type == crate::pipeline::foreman::FOREMAN_AGENT_TYPE
            }
            SseEvent::ForemanProposal { .. } => true,
            _ => false,
        }
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
            | SseEvent::ForemanProposal { task_id, .. }
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

/// 会话增量走哪条声道（决策 244）。
///
/// **两条声道必须分开**：`content` 是模型要说的话，`reasoning` 是它的草稿。
/// 合成一条之后，界面分不出哪一段该当回话念、哪一段该收进折叠块——而「把思考当回话
/// 念给人听」与「把回话折起来看不见」都是错的。
///
/// 默认是 `Content`（`serde(default)`）：老客户端收到没有 `channel` 字段的事件会照旧
/// 把它当回话文本拼上去，那正是它们此前唯一见过的形状。加字段是**加性**改动。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// 回话正文（今天已有的那一类）。
    #[default]
    Content,
    /// 推理 / 思考增量。只走实时通道，不落库、不回灌（决策 244）。
    Reasoning,
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
                channel: Channel::Content,
                role: "assistant".into(),
                text: "正在比对".into(),
                prompt_tokens: 3,
                completion_tokens: 4,
            },
            SseEvent::ForemanProposal {
                task_id: String::new(),
                branch: String::new(),
                session_id: "s1".into(),
                proposal_id: "p1".into(),
                tool: "write_file".into(),
                status: "pending".into(),
                summary: "写入 notes.md".into(),
                expires_at: "2026-09-17T00:10:00+00:00".into(),
            },
            SseEvent::ToolEvent {
                task_id: "t".into(),
                branch: "main".into(),
                run_id: 7,
                agent_type: "main".into(),
                session_id: String::new(),
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
            channel: Channel::Content,
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
            // 决策 244：回话 / 思考两条声道的分道标记。
            "channel",
            "role",
            "text",
            "prompt_tokens",
            "completion_tokens",
        ] {
            assert!(json.get(key).is_some(), "conversation_delta 缺少 {key}");
        }
        assert_eq!(json["type"], "conversation_delta");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["channel"], "content");
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

    /// 决策 244：`channel` 是**加性**字段，缺省落在 `content` 那一侧。
    ///
    /// 默认值的方向不是随便定的：老客户端收到没有 `channel` 的增量时会把它当回话正文拼上去
    /// ——那正是这条通道在加字段之前唯一见过的形状。默认成 `reasoning` 会让它们**静默丢掉
    /// 每一段回话**。
    #[test]
    fn conversation_delta_channel_defaults_to_content() {
        let raw = r#"{"type":"conversation_delta","task_id":"t","branch":"main","run_id":1,
                       "agent_type":"main","session_id":"","role":"assistant","text":"hi",
                       "prompt_tokens":0,"completion_tokens":0}"#;
        let ev: SseEvent = serde_json::from_str(raw).unwrap();
        match ev {
            SseEvent::ConversationDelta { channel, .. } => assert_eq!(channel, Channel::Content),
            other => panic!("解成了别的变体：{other:?}"),
        }

        // 显式给 reasoning 时要真的读出来（并且序列化成 snake_case）
        let thought = SseEvent::ConversationDelta {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
            session_id: String::new(),
            channel: Channel::Reasoning,
            role: "assistant".into(),
            text: "想想".into(),
            prompt_tokens: 0,
            completion_tokens: 0,
        };
        let json: serde_json::Value = serde_json::from_str(&thought.to_json()).unwrap();
        assert_eq!(json["channel"], "reasoning");
    }

    /// 决策 244：工具事件带身份串，于是它走**同一个** `is_foreman_event` 过滤。
    ///
    /// 这一条是诉求「工具调用不要等对话完结才展示」的落点：没有身份串，那个判定无法把
    /// 值班长的工具调用与流水线节点的区分开，痕迹就只能等那一轮落库后从 `traces_json` 读。
    #[test]
    fn tool_events_are_routed_to_the_foreman_stream_by_identity() {
        let foreman_tool = SseEvent::ToolEvent {
            task_id: String::new(),
            branch: String::new(),
            run_id: 0,
            agent_type: "foreman".into(),
            session_id: "s1".into(),
            tool: "read_task".into(),
            phase: ToolPhase::Start,
            args_summary: "t-1".into(),
        };
        assert!(
            foreman_tool.is_foreman_event(),
            "值班长的工具调用该走对讲台流"
        );
        let json: serde_json::Value = serde_json::from_str(&foreman_tool.to_json()).unwrap();
        assert_eq!(json["agent_type"], "foreman");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["phase"], "start");

        // 流水线节点的工具调用照旧不进对讲台（身份串对不上）
        for agent_type in ["main", "system", "pseudo:project_analysis"] {
            let pipeline_tool = SseEvent::ToolEvent {
                task_id: "t".into(),
                branch: "main".into(),
                run_id: 7,
                agent_type: agent_type.into(),
                session_id: String::new(),
                tool: "write_file".into(),
                phase: ToolPhase::End,
                args_summary: "x".into(),
            };
            assert!(
                !pipeline_tool.is_foreman_event(),
                "{agent_type} 的工具事件不该进对讲台"
            );
        }

        // 老后端不发 agent_type：缺省空串不等于 "foreman"，即「认不出来就当作别人的」
        let legacy = r#"{"type":"tool_event","task_id":"t","branch":"main","run_id":7,
                        "tool":"write_file","phase":"end","args_summary":"x"}"#;
        let ev: SseEvent = serde_json::from_str(legacy).unwrap();
        match &ev {
            SseEvent::ToolEvent {
                agent_type,
                session_id,
                ..
            } => {
                assert_eq!(agent_type, "");
                assert_eq!(session_id, "");
            }
            other => panic!("解成了别的变体：{other:?}"),
        }
        assert!(!ev.is_foreman_event());
    }

    /// 决策 207：提议事件走 `/foreman/stream`，而流水线的工具事件不走——两条都要钉住，
    /// 否则「对讲台看得到提议」与「任务页不混进值班长的事」里必有一条是碰巧成立的。
    #[test]
    fn proposal_events_reach_the_foreman_stream_only() {
        let proposal = SseEvent::ForemanProposal {
            task_id: String::new(),
            branch: String::new(),
            session_id: "s1".into(),
            proposal_id: "p1".into(),
            tool: "write_file".into(),
            status: "pending".into(),
            summary: "写入 notes.md".into(),
            expires_at: "2026-09-17T00:10:00+00:00".into(),
        };
        assert!(proposal.is_foreman_event());
        assert_eq!(proposal.event_type(), SseEventType::ForemanProposal);
        assert!(!SseEvent::ToolEvent {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
            session_id: String::new(),
            tool: "write_file".into(),
            phase: ToolPhase::Start,
            args_summary: "x".into(),
        }
        .is_foreman_event());
        let pipeline_delta = SseEvent::ConversationDelta {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
            session_id: String::new(),
            channel: Channel::Content,
            role: "assistant".into(),
            text: "x".into(),
            prompt_tokens: 0,
            completion_tokens: 0,
        };
        assert!(!pipeline_delta.is_foreman_event());
    }

    #[test]
    fn tool_event_fields_complete() {
        let ev = SseEvent::ToolEvent {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
            session_id: String::new(),
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
