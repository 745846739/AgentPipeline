use std::collections::HashMap;

use crate::storage::foreman::ForemanMessage;

/// 历史窗口的字符预算（决策 182⑫）。
///
/// **按字符裁剪而不是硬编码轮数**：一轮的长短差两个数量级（「在吗」3 字 vs 贴一段回执
/// 2000 字），按轮数裁会让长轮挤出上下文、短轮浪费预算。被裁掉的历史仍在库里（`list_foreman_messages`
/// 只影响这一轮注入了什么，不影响台账）。
pub const FOREMAN_HISTORY_BUDGET_CHARS: usize = 24_000;

/// 上下文压缩（决策 269 / 票 foreman-within-boundary 03）的锚点前缀标记。
/// 锚点以**一条 user 轮**的形态带头拼进历史头部（system 行重注入走 user 的先例，
/// 204 同姿态）；测试按它认锚点（`over_budget_history_is_summarized_…`）。
pub const COMPACTION_MARK: &str = "【更早的对话已压缩成下面这段摘要——原始轮次仍在班次台账里】";

/// 跨时间线互喂的两个标记（决策 289 / 票 03）：与 [`COMPACTION_MARK`] 同族——都是
/// 「以一条带标记的 user 轮注入的摘要」，进模型上下文才拦得住，落库之后还能审计
/// （摘要本身不落库，落库的是它各自的原始轮次）。
/// 人的那一轮读到**值守台账的摘要**：值班经理指着播报说「处理一下」时，模型知道
/// 说的是哪一件——而它看到的是摘要，不是整本流水账（预算的另一半不吃）。
pub const FOREMAN_WATCH_DIGEST_MARK: &str = "【值守摘要】";
/// 值守轮读到**人的对话的摘要**（裁决 2：它仍读得到人说的话——以摘要形态）。
pub const FOREMAN_TALK_DIGEST_MARK: &str = "【人的对话摘要】";

/// 摘要器的专用指令（269②：同一 `llm.complete`、同 provider 的**无工具**小补全）。
pub(super) const SUMMARIZER_SYSTEM_PROMPT: &str =
    "你是对话历史压缩器。把给定的历史压成**一段摘要**，\
    作为值班长后续轮次的上下文锚点。保留：任务与结论、关键报错与证据原文（尽量短）、\
    值班经理给过的方向与决定、未决问题与承诺。丢弃：寒暄、重复、过程细节。\
    只输出摘要正文——不要标题、不要解释、不要列表符号。";
/// 摘要调用的绝对上限：它是主轮之外的一次附加调用，挂住不能把整轮拖死
/// （超时即回退现状，269④）。
pub(super) const SUMMARIZER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 摘要输出上限（锚点自身要能进预算算术，269②）。
pub(super) const SUMMARIZER_MAX_TOKENS: u32 = 800;
pub(super) const SUMMARIZER_MAX_CHARS: usize = 4_000;
/// 摘要输入上限：掉出预算的区间可能很长，超限**留尾巴**（离窗口最近的轮次最要紧）。
pub(super) const SUMMARIZER_INPUT_MAX_CHARS: usize = 60_000;
/// 锚点缓存的会话槽数（FIFO 淘汰；重启重算可接受——锚点不落库，269③）。
const COMPACTION_CACHE_SLOTS: usize = 16;

/// 一条会话的压缩缓存（决策 269③）：`covered_until` = 摘要已覆盖到的消息 id
/// （第一条**没**被覆盖的那条）。窗口边界在会话内单调前进，下一轮从这里接着增量压
/// ——每条轮次一生只被压一次，最老原文不整段重发。
#[derive(Clone)]
pub(super) struct CompactionEntry {
    pub(super) covered_until: i64,
    pub(super) summary: String,
}

/// 缓存本体：按会话分槽、FIFO 淘汰（上限 [`COMPACTION_CACHE_SLOTS`]）。
#[derive(Default)]
pub(super) struct CompactionCache {
    entries: HashMap<String, CompactionEntry>,
    order: std::collections::VecDeque<String>,
}

impl CompactionCache {
    pub(super) fn get(&self, session: &str) -> Option<&CompactionEntry> {
        self.entries.get(session)
    }

    pub(super) fn put(&mut self, session: &str, entry: CompactionEntry) {
        if !self.entries.contains_key(session) {
            while self.order.len() >= COMPACTION_CACHE_SLOTS {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.order.push_back(session.to_string());
        }
        self.entries.insert(session.to_string(), entry);
    }
}

/// 摘要器的输入（269③）：已有摘要打头（增量）+ 新掉出预算的轮次逐条带发言者；
/// 整段超限时留尾巴——首条掉队的原文从此只活在旧摘要里，不整段重发。
pub(super) fn summarize_input(newly: &[ForemanMessage], prefix: Option<&str>) -> String {
    let mut out = String::new();
    match prefix {
        Some(p) => {
            out.push_str("【已有摘要】\n");
            out.push_str(p);
            out.push_str("\n\n【新掉出预算的轮次】\n");
        }
        None => out.push_str("【掉出预算的对话历史】\n"),
    }
    for m in newly {
        let who = if m.role == crate::storage::foreman::FOREMAN_ROLE_USER {
            "值班经理"
        } else if m.role == crate::storage::foreman::FOREMAN_ROLE_SYSTEM {
            "操作台"
        } else {
            "值班长"
        };
        out.push_str(&format!("[{who}]\n{}\n\n", m.content));
    }
    let n = out.chars().count();
    if n > SUMMARIZER_INPUT_MAX_CHARS {
        let kept: String = out.chars().skip(n - SUMMARIZER_INPUT_MAX_CHARS).collect();
        format!("（更早部分略）\n{kept}")
    } else {
        out
    }
}

/// 一次性从库里取出的历史行数上限——真正的裁剪判据是字符预算，
/// 这个数字只是「别把整晚的对话都读进内存」的粗兜底。
pub(super) const FOREMAN_HISTORY_FETCH_LIMIT: usize = 200;

/// 单条工具结果回灌进对话前的截断上限（字符）。
///
/// 压在 `offload_threshold_tokens`（默认 4000 token ≈ 16000 字符）**之下**：
/// 值班长的工具结果不该走 L2 卸载——那条路会把内容写到 `context_dir/{task_id}`，
/// 而值班长没有 task_id。截断比卸载诚实：模型看到的是「这里有 12000 字，这是全部」。
pub(crate) const FOREMAN_TOOL_RESULT_MAX_CHARS: usize = 12_000;

/// `read_conversation` 返回的最后 N 条消息、以及它们的总字符上限。
///
/// 公开给 crate 内是因为真正的截断发生在工具实现（`agent::tools`）里——两处各写一份迟早
/// 漂移，而漂移的后果是「值班长读到的回执比它以为的长」这种不显眼的超支。
pub(crate) const FOREMAN_CONVERSATION_MAX_MESSAGES: usize = 20;
pub(crate) const FOREMAN_CONVERSATION_MAX_CHARS: usize = 12_000;

/// 历史窗口按字符预算裁剪（决策 182⑫）：从**最新往回**取，返回选中项（时间升序）。
///
/// 两条不变式：
/// - **最新一条一定在内**（哪怕它自己就超预算）——它是本轮刚说出口的那句话，
///   把它裁掉会让值班长答非所问；
/// - 被裁掉的历史**仍留在库里**，只是这一轮没进 prompt。
pub fn trim_history(history: &[ForemanMessage], budget_chars: usize) -> Vec<ForemanMessage> {
    let mut selected: Vec<ForemanMessage> = Vec::new();
    let mut used = 0usize;
    for msg in history.iter().rev() {
        let cost = msg.content.chars().count();
        if !selected.is_empty() && used + cost > budget_chars {
            break;
        }
        used += cost;
        selected.push(msg.clone());
    }
    selected.reverse();
    selected
}
