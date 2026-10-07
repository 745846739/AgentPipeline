//! 节点内消息日志（.scratch/node-message-resume 票 01）：agent 节点主循环的**逐条**转录。
//!
//! 与 `observability` 里的会话表（`kanban_node_conversations`）是**两件事**，不是一个东西的
//! 两种写法：
//!
//! | | 会话表（观测归档） | 本表（新增，节点内 checkpoint） |
//! |---|---|---|
//! | 写入时机 | 循环退出之后一次性写成一整块 | 每产生一条消息就落一行，**边跑边写** |
//! | 压缩 | `truncate_messages_json` 从最旧一端整条丢 | 一个字不碰（只追加，无压缩事件） |
//! | 读者 | 任务详情页 / 诊断包 | `take_continuation`（续接素材） |
//!
//! **本表的存在理由是会话表做不到的另一件事**：服务被强杀时，会话表里那一条**根本还没写**
//! （它写于循环退出之后），于是中断的节点只能从零重跑、把做过的工具副作用再做一遍。本表把
//! 「最后一条已记录的消息」变成恢复起点的素材。
//!
//! **每个 run 自包含**：一次 attempt 起跑时把承接的转录前缀批量写进来（seq 从 0 起），
//! 之后每条新消息续着写。于是「某 run 的全部行」= 那次尝试的完整转录。
//!
//! 表的语义与索引见迁移 `0044_node_messages.sql`。

use crate::agent::client::{Message, Role};
use crate::types::{Node, Stage};
use crate::Result;

use super::{ts, Store};

/// 主 agent 的行（续接查找键的第三个分量，与 `latest_own_conversation` 同值）。
pub const AGENT_TYPE_MAIN: &str = "main";

/// 一个 run 的完整转录（本表按 `(run_id, seq)` 还原）。
pub struct NodeTranscript {
    pub run_id: i64,
    pub messages: Vec<Message>,
}

/// `kanban_node_messages` 的一行（读侧）。
#[derive(sqlx::FromRow)]
struct MessageRow {
    role: String,
    content: Option<String>,
    tool_calls_json: Option<String>,
    tool_call_id: Option<String>,
    name: Option<String>,
}

impl MessageRow {
    fn into_message(self) -> Result<Message> {
        // 工具调用是**原始参数串**（`ToolCallWire`：`function.arguments` 是 JSON 字符串），
        // 反序列化不碰它——逐字段还原时原样交回 provider。
        let tool_calls = match self.tool_calls_json.as_deref() {
            Some(raw) if !raw.is_empty() => serde_json::from_str(raw)?,
            _ => Vec::new(),
        };
        let role = Role::parse(&self.role).ok_or_else(|| {
            crate::Error::Validation(format!("节点消息日志里出现未知角色：{}", self.role))
        })?;
        Ok(Message {
            role,
            content: self.content,
            tool_calls,
            tool_call_id: self.tool_call_id,
            name: self.name,
        })
    }
}

impl Store {
    /// 一条消息 → 一行。
    ///
    /// 单条 INSERT（不批、不延迟）是**承重**的：调用方（`model_invoke` 的轮循环）依赖
    /// 「assistant 行在写侧已经提交，才去执行这一批工具」这个顺序——崩溃时最多丢一条
    /// 还没落库的模型响应，而绝不会出现「工具跑了、日志里没有它要回执的那条 assistant」。
    ///
    /// 参数多于 clippy 默认阈值：那六个身份字段（任务 / run / 阶段 / 节点 / agent_type /
    /// 序号）就是这张表的**坐标**，绑成结构体只是把同一份信息换个地方写，改不了调用点的
    /// 可读性（与 `insert_conversation` / `append_node_messages` 同一处置）。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_node_message(
        &self,
        task_id: &str,
        run_id: i64,
        stage: Stage,
        node: Node,
        agent_type: &str,
        seq: i64,
        message: &Message,
        synthetic: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO kanban_node_messages
             (run_id, task_id, stage, node, agent_type, seq, role, content, tool_calls_json,
              tool_call_id, tool_name, synthetic, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(task_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(agent_type)
        .bind(seq)
        .bind(message.role.as_str())
        .bind(message.content.as_deref())
        .bind(tool_calls_json(message)?)
        .bind(message.tool_call_id.as_deref())
        .bind(message.name.as_deref())
        .bind(i64::from(synthetic))
        .bind(ts(self.now()))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 一批消息 → 一组行（attempt 起跑时写承接前缀用；`seq` 从 `start_seq` 起递增）。
    ///
    /// `synthetic_from` 是**合成回执的起始下标**（票 02）：补齐的回执是承接转录末尾的一段连续
    /// 区间，`from` 之后的都算合成件。**按下标而不是「末尾几条」**：调用方在写前缀之前还会往
    /// 转录末尾追加「补充输入 / 评审打回反馈」的 user turn（决策 279 / 387），那些是**真消息**——
    /// 按尾部计数会把标记打错人（把真 turn 标成合成的、把合成回执标成真的）。
    ///
    /// 一个写事务：前缀可能有几十上百行，中途失败留下半份前缀会让续接拿到一份**看着完整、
    /// 实则截断**的转录——那正是本表要根除的那类失真。
    #[allow(clippy::too_many_arguments)]
    pub async fn append_node_messages(
        &self,
        task_id: &str,
        run_id: i64,
        stage: Stage,
        node: Node,
        agent_type: &str,
        start_seq: i64,
        messages: &[Message],
        synthetic_from: Option<usize>,
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let now = ts(self.now());
        let mut tx = self.begin_write().await?;
        for (offset, message) in messages.iter().enumerate() {
            sqlx::query(
                "INSERT INTO kanban_node_messages
                 (run_id, task_id, stage, node, agent_type, seq, role, content, tool_calls_json,
                  tool_call_id, tool_name, synthetic, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(run_id)
            .bind(task_id)
            .bind(stage.as_str())
            .bind(node.as_str())
            .bind(agent_type)
            .bind(start_seq + offset as i64)
            .bind(message.role.as_str())
            .bind(message.content.as_deref())
            .bind(tool_calls_json(message)?)
            .bind(message.tool_call_id.as_deref())
            .bind(message.name.as_deref())
            .bind(i64::from(synthetic_from.is_some_and(|from| offset >= from)))
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 某 `(task, stage, node)` 上**主 agent 自己**最近一次尝试的转录（票 02）。
    ///
    /// 查找键与 [`Store::latest_own_conversation`] **同源**（决策 180 的「自己」口径）：
    /// `agent_type = 'main'` 过滤掉伪阶段与子代理——它们复用父节点的 stage/node（决策 172），
    /// 不过滤会把伪阶段的对话当成上一轮读回来。
    ///
    /// **不能按游标找**：`goto` 是在**同一条游标行**上改 `(stage, node)`，按 `cursor_id` 找
    /// 会把上一个节点的对话喂给这个节点。
    ///
    /// 没有行时给 `None`：那是「这个节点还没在这个库上跑过任何一条消息」（首次进入，或
    /// 库是本次升级前建的）——调用方按干净起跑处置，不报错。
    pub async fn latest_own_transcript(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
    ) -> Result<Option<NodeTranscript>> {
        let run_id: Option<i64> = sqlx::query_scalar(
            "SELECT run_id FROM kanban_node_messages
             WHERE task_id = ? AND stage = ? AND node = ? AND agent_type = ?
             ORDER BY run_id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(AGENT_TYPE_MAIN)
        .fetch_optional(self.pool())
        .await?;
        let Some(run_id) = run_id else {
            return Ok(None);
        };
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT role, content, tool_calls_json, tool_call_id, tool_name AS name
             FROM kanban_node_messages WHERE run_id = ? ORDER BY seq",
        )
        .bind(run_id)
        .fetch_all(self.pool())
        .await?;
        let messages = rows
            .into_iter()
            .map(MessageRow::into_message)
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(NodeTranscript { run_id, messages }))
    }

    /// 某 run 的消息行数（测试与排查用；读侧不许拿它当「转录完整」的判据）。
    pub async fn count_node_messages(&self, run_id: i64) -> Result<i64> {
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM kanban_node_messages WHERE run_id = ?")
                .bind(run_id)
                .fetch_one(self.pool())
                .await?;
        Ok(n)
    }

    /// 某 run 的合成回执行数（票 02 的取证面）。
    pub async fn count_synthetic_node_messages(&self, run_id: i64) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_node_messages WHERE run_id = ? AND synthetic = 1",
        )
        .bind(run_id)
        .fetch_one(self.pool())
        .await?;
        Ok(n)
    }
}

/// 工具调用的落库形态：`ToolCallWire` 的 JSON 数组，**参数串原样**（不做二次序列化）。
/// 没有工具调用时给 `None`（NULL = 「这条消息没带调用」，与「带了个空数组」是同一件事，
/// 但少一份无意义的 JSON）。
fn tool_calls_json(message: &Message) -> Result<Option<String>> {
    if message.tool_calls.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&message.tool_calls)?))
}
