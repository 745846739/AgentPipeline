//! 断言助手（决策 146）：游标状态、run / 命令行数、SSE 事件录制。

use std::sync::{Arc, Mutex};

use agentpipeline_core::sse::{SseEvent, SseEventType, SseSink};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{CursorStatus, Node, NodeCursor, Stage};
use agentpipeline_core::Result;

/// 游标停在某阶段某节点。
pub fn assert_cursor_at(cursor: &NodeCursor, stage: Stage, node: Node) {
    assert_eq!(cursor.stage, stage, "游标 stage 不符：{:?}", cursor.stage);
    assert_eq!(cursor.node, node, "游标 node 不符：{:?}", cursor.node);
}

/// SSE 事件录制器（testing.md §3.3 的断言助手之一）。
#[derive(Default, Clone)]
pub struct SseRecorder {
    events: Arc<Mutex<Vec<SseEvent>>>,
}

impl SseRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<SseEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn count_of(&self, kind: SseEventType) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.event_type() == kind)
            .count()
    }

    pub fn last_of(&self, kind: SseEventType) -> Option<SseEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|e| e.event_type() == kind)
            .cloned()
    }

    /// 事件触及的分支集合。
    pub fn branches(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.branch().to_string())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// 某类型事件的 `type` 字符串序列（断言事件顺序用）。
    pub fn type_sequence(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| {
                serde_json::to_value(e.event_type())
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            })
            .collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl SseSink for SseRecorder {
    fn emit(&self, event: SseEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// 该任务的 run 行数（含 system）。
pub async fn run_count(store: &Store, task_id: &str) -> Result<usize> {
    Ok(store.list_runs(task_id).await?.len())
}

/// 该任务调用 LLM 的 run 行数（排除 `agent_type = "system"`，决策 130 ②）。
pub async fn llm_run_count(store: &Store, task_id: &str) -> Result<u64> {
    let runs = store.list_runs(task_id).await?;
    Ok(agentpipeline_core::metrics::total_calls(&runs))
}

/// 该任务的命令日志条数。
pub async fn command_count(store: &Store, task_id: &str) -> Result<usize> {
    Ok(store.list_commands(task_id, None, None).await?.len())
}

/// 活跃 / 归档游标数（同时验证"游标行永不物理删除"，决策 113）。
pub async fn cursor_counts(store: &Store, task_id: &str) -> Result<(usize, usize)> {
    let all = store.load_all_cursors(task_id).await?;
    let live = all
        .iter()
        .filter(|c| c.status != CursorStatus::Archived)
        .count();
    Ok((live, all.len() - live))
}

/// 分支上的活跃游标。
pub async fn live_cursor_for_branch(
    store: &Store,
    task_id: &str,
    branch: &str,
) -> Result<Option<NodeCursor>> {
    Ok(store
        .load_live_cursors(task_id)
        .await?
        .into_iter()
        .find(|c| c.branch == branch))
}
