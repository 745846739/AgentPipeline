use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::sse::Channel;
use crate::storage::foreman::{InFlightPatch, NewForemanMessage};
use crate::storage::Store;
use crate::Result;

use super::*;

/// 进程内「按班次计数」的轮次登记表（决策 351 把决策 260 的 `FOREMAN_TURNS` 机制化）。
///
/// 为什么需要它：一轮回话跑在独立任务里（决策 223），**它不随请求一起死**——本地放弃只
/// 丢掉这一次的同步回包，回话照旧落库。可这条实情此前只写在文案里（超时那一类的
/// `failureNotice`），**没有任何读数**：刷新页面之后，界面上既没有「这一轮还在跑」的
/// 那一轮（乐观轮与流式轮都住在 `sending` 这把局部状态里），也就**不再累积增量**——
/// 于是「它在说话」这件事只有等回话落地、重读台账才看得见。本节补的正是这个读数。
///
/// 登记的是**会话 id**、不是「一轮」的标识：界面要知道的是「这一班此刻有一轮在跑」，
/// 它据此才敢把到达的增量接进时间线（否则那一段字属于谁就无从判断）。
///
/// **进程内**（照 [`crate::pipeline::executor`] 的 `EXECUTOR_REGISTRY` 同一姿态）：跨进程
/// 的情形是**上一个实例留下的**，那种「还在跑」是假的——而它恰好由启动时的
/// `orphan_inflight_model_requests` 与 `requeue_running_tasks` 一起收口（决策 255④ / 226）。
///
/// **计数纪律写进类型接口**（[`Self::begin`] 发凭据、凭据 Drop 即摘、[`Self::in_flight`]
/// 读数）：记账用**计数**而不是一个布尔 / 一格代次——同一班**可以**同时跑着两轮，值守轮
/// （决策 209）与人打的一句话各起一轮，两者之间没有任何互斥。按格覆盖的话，先退出的那一轮
/// 会把另一轮的登记一起摘掉，界面于是在真的还在跑的时候读到「不在跑了」；计数不会：它就是
/// 「此刻有几轮在跑」这个读数本身。该不变量由本模块的单测钉住（bool 错法是真踩过的坑）。
struct TurnRegistry {
    counts: Mutex<HashMap<String, usize>>,
}

/// [`TurnRegistry`] 的登记凭据：**退出即摘**（与 [`HumanTurnGuard`] / [`TurnCancelGuard`]
/// 同一姿态）。
pub(super) struct TurnGuard {
    registry: &'static TurnRegistry,
    key: String,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

impl TurnRegistry {
    /// 登记「这个 key 开始跑一轮」，返回退出即自动摘的凭据。
    fn begin(&'static self, key: &str) -> TurnGuard {
        *self
            .counts
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_insert(0) += 1;
        TurnGuard {
            registry: self,
            key: key.to_string(),
        }
    }

    fn release(&self, key: &str) {
        let mut counts = self.counts.lock().unwrap();
        match counts.get_mut(key) {
            Some(count) if *count > 1 => *count -= 1,
            _ => {
                counts.remove(key);
            }
        }
    }

    fn in_flight(&self, key: &str) -> bool {
        self.counts.lock().unwrap().contains_key(key)
    }
}

/// 此刻**有哪几班正在跑一轮**（决策 260）。
static FOREMAN_TURNS: LazyLock<TurnRegistry> = LazyLock::new(|| TurnRegistry {
    counts: Mutex::new(HashMap::new()),
});

/// 登记「这一班开始跑一轮」，返回退出即自动摘的凭据（决策 260）。
pub(super) fn begin_foreman_turn(session_id: &str) -> TurnGuard {
    FOREMAN_TURNS.begin(session_id)
}

/// 此刻这一班**有一轮在跑**吗（决策 260）。
///
/// 界面刷新之后靠它决定「要不要把到达的增量接进时间线」——见 [`FOREMAN_TURNS`]。
/// 读的是**登记**而不是台账：台账里没有「在跑」这一行（回话落库才算数），而这一轮的
/// 现场（乐观轮 / 流式文本）本来就全在界面那侧，刷新即丢。
pub fn foreman_turn_in_flight(session_id: &str) -> bool {
    FOREMAN_TURNS.in_flight(session_id)
}

// ─────────── 在途半截行的现场（票 01，spec .scratch/talk-replay 决策 1 / 3 / 5）───────────

/// 节流拍（接缝窗口 ≲300ms）：两次刷库之间至少隔这么久，攒下的增量下拍一起写。
///
/// **实现细节，不是可测契约**（spec Testing Decisions：不断言内部调用了几次批写）。
/// 取 250ms 是给 300ms 目标留的余量——重连时最多回退这么久没落库的字，而那部分字
/// 本来也没进库，与决策 275（不回放、只 refetch 校准）相容。收口那一下**不受它管**：
/// 终态永远以收口写为准，节流只管中途。
const LIVE_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// 一轮在飞时**台账那半截行的进程内现场**（票 01）。
///
/// 一份现场、两个写者、一个读口：
/// - [`Self::observe_call`] / [`Self::settle_round`]：`respond_inner` 在模型调用的边界上
///   给**权威值**（段序 / 痕迹 / 累计推理 / token）整体覆盖；
/// - [`push_foreman_live_delta`]：provider 把逐字正文与推理增量推进来——「随广播落库」
///   的字面意思，流式途中每 250ms 落一次（[`flush_foreman_live_turn`]）。
///
/// 读口只有 [`Store::update_foreman_inflight`]：一次 UPDATE 写整行，`WHERE status =
/// 'in_flight'` 让迟到的刷写改不动收口后的终态。
///
/// **进程内登记**（与 [`FOREMAN_TURNS`] 同姿态）：流式增量按 `session_id` 找现场，故
/// 这里也按班次登记；同班次撞上第二轮时后者顶掉前者的流式入口（两个现场各写各的行，
/// 只有逐字增量这一个读口会串——同班两轮并跑本就是罕见形态）。
pub struct LiveTurn {
    session_id: String,
    row_id: i64,
    store: Store,
    state: Mutex<LiveState>,
}

#[derive(Default)]
struct LiveState {
    /// 已收场的段序（权威，由 `respond_inner` 整体覆盖）。
    segments: Vec<ForemanSegment>,
    /// 已收场的推理（权威累计：跨调用按 `\n\n` 拼的那份）。
    thinking_done: String,
    /// 正在冒的**这一次调用**的推理增量（收场时并进 `thinking_done` 并清零）。
    thinking_live: String,
    /// 正在冒的**这一次调用**的正文：收口那句的半成品（收口时被权威 `content` 换掉，
    /// 中途挪进段序时清零）。
    content_live: String,
    /// 工具痕迹聚合（权威，随 `segments` 一起在调用边界上覆盖）。
    traces: Vec<ForemanTrace>,
    /// 这一轮到目前的 `(prompt, completion)` token（调用边界上更新）。
    tokens: (u32, u32),
    /// **行内位置序号**（票 02）：每一段进现场的增量 +1，刷库时原样写进 `seq` 列——
    /// 它因此**只数已经进现场的东西**（见 [`LiveState::to_patch`]）。
    seq: u64,
    /// **已广播、尚未进场**的位置（票 02）：工具事件在**发射那一刻**就要一个位置号
    /// （它广播给了界面），但它的段序要等这一批工具跑完才进现场。刷库时不写它
    /// ——否则快照会声称覆盖了一个其实还没有的事件，接缝处就丢字。
    /// 收场时并进 `seq`（段序进现场了），见 [`Self::settle_round`]。
    reserved: u64,
    /// 上次真写库的时刻（节流拍）。`None` = 还没写过——第一拍不等（第一个字尽快落地）。
    last_flush: Option<std::time::Instant>,
    /// 有东西还没写（被节流跳过的增量）。
    dirty: bool,
    /// 已收口 / 已丢弃：迟到的刷写一律空操作。
    finished: bool,
}

impl LiveState {
    /// 现场 → 一次刷库的载荷。**纯函数**：序列化失败不改状态（`dirty` 留给下次）。
    fn to_patch(&self) -> Result<InFlightPatch> {
        // 「正在冒」的那两段以**尾段**的形式并进段序：重进后读到的半截行与当时直播
        // 所见同构（spec 决策 3 全保真）——推理是折叠块、正文是回话位，两者都不丢。
        let mut segments = self.segments.clone();
        if !self.thinking_live.is_empty() {
            segments.push(ForemanSegment::Thinking {
                text: self.thinking_live.clone(),
            });
        }
        let mut thinking = self.thinking_done.clone();
        if !self.thinking_live.is_empty() {
            if !thinking.is_empty() {
                thinking.push_str("\n\n");
            }
            thinking.push_str(&self.thinking_live);
        }
        Ok(InFlightPatch {
            content: self.content_live.clone(),
            thinking: (!thinking.trim().is_empty()).then_some(thinking),
            segments_json: if segments.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&segments)?)
            },
            traces_json: if self.traces.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&self.traces)?)
            },
            prompt_tokens: self.tokens.0,
            completion_tokens: self.tokens.1,
            seq: self.seq,
        })
    }
}

/// 按班次登记的在飞现场（见 [`LiveTurn`] 的说明）。
static FOREMAN_LIVE_TURNS: LazyLock<Mutex<HashMap<String, Arc<LiveTurn>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl LiveTurn {
    /// **一轮开工**：台账里当场建这条回话的半截行（spec 决策 1），并登记现场。
    pub(super) async fn begin(
        store: Store,
        session_id: &str,
        briefing_json: Option<serde_json::Value>,
    ) -> Result<Arc<LiveTurn>> {
        let row_id = store
            .begin_foreman_inflight(session_id, briefing_json)
            .await?;
        let turn = Arc::new(LiveTurn {
            session_id: session_id.to_string(),
            row_id,
            store,
            state: Mutex::new(LiveState::default()),
        });
        if FOREMAN_LIVE_TURNS
            .lock()
            .unwrap()
            .insert(session_id.to_string(), turn.clone())
            .is_some()
        {
            tracing::warn!(
                session_id,
                "同一班次已有在飞的半截行现场：后来者顶掉流式入口（两行各写各的，只有逐字增量串）"
            );
        }
        Ok(turn)
    }

    /// 流式增量（正文 / 推理）推进现场。**同步**：它挂在 `emit_delta` 上，逐字到达的
    /// 频率容不下一次锁之外的任何开销，写库交给节流的那一拍。
    ///
    /// 返回这一段的**位置戳** `(行 id, seq)`（票 02），由发射方挂进事件——增量与它的字
    /// **同时**进现场，故 `seq` 刷进库里时它必然已被覆盖（快照不会漏掉它）。
    /// `None` = 现场不在（已收口 / 没登记）：事件照发、不带戳，前端按老路径接。
    fn push_delta(&self, channel: Channel, text: &str) -> Option<(i64, u64)> {
        let mut st = self.state.lock().unwrap();
        if st.finished || text.is_empty() {
            return None;
        }
        match channel {
            Channel::Content => st.content_live.push_str(text),
            Channel::Reasoning => st.thinking_live.push_str(text),
        }
        st.seq += 1;
        st.dirty = true;
        Some((self.row_id, st.seq))
    }

    /// **一个工具事件的发射位**（票 02）：先领一个位置号再广播，但**不进现场**
    /// （段序要等这一批工具跑完才落）。故它此后第一次刷库时**不会**被写进 `seq`——
    /// 快照不会声称覆盖一个还没有的事件；收场时段序进场，`settle_round` 把它并进去。
    pub(super) fn reserve_event_seq(&self) -> (i64, u64) {
        let mut st = self.state.lock().unwrap();
        st.reserved = st.reserved.max(st.seq) + 1;
        (self.row_id, st.reserved)
    }

    /// **一次模型调用收齐了**（`respond_inner` 在调用边界上叫）：段序、累计推理与 token
    /// 是此刻的权威值，整体覆盖；这一次调用的推理增量已经并进 `thinking_done`，清零。
    ///
    /// 正文**不清**：它此刻要么还在 `content_live` 里（还没进段序），要么马上作为
    /// 中途的 `Text` 段进段序——两种时态之间没有刷库点，不会两处都显示。
    pub(super) fn observe_call(
        &self,
        segments: &[ForemanSegment],
        thinking: &str,
        tokens: (u32, u32),
    ) {
        let mut st = self.state.lock().unwrap();
        st.segments = segments.to_vec();
        st.thinking_done = thinking.to_string();
        st.thinking_live.clear();
        st.tokens = tokens;
        st.dirty = true;
    }

    /// **一轮迭代收场**（工具都跑完了）：段序与痕迹此刻含这一轮的全部已收场步骤，
    /// 「正在冒」的两段并了进去、清零——下一次调用的增量从干净的底子上长。
    pub(super) fn settle_round(
        &self,
        segments: &[ForemanSegment],
        thinking: &str,
        traces: &[ForemanTrace],
        tokens: (u32, u32),
    ) {
        self.observe_call(segments, thinking, tokens);
        let mut st = self.state.lock().unwrap();
        st.traces = traces.to_vec();
        st.content_live.clear();
        // 段序进现场了：这一批工具事件的位置号从此**被覆盖**（刷库可以写它们）。
        st.seq = st.seq.max(st.reserved);
        st.reserved = 0;
        st.dirty = true;
    }

    /// 节流刷库（`force` = 调用边界上不等拍）。
    pub(super) async fn flush(&self, force: bool) -> Result<()> {
        let patch = {
            let mut st = self.state.lock().unwrap();
            if st.finished || !st.dirty {
                return Ok(());
            }
            if !force
                && st
                    .last_flush
                    .is_some_and(|t| t.elapsed() < LIVE_FLUSH_INTERVAL)
            {
                return Ok(());
            }
            let patch = st.to_patch()?;
            st.dirty = false;
            st.last_flush = Some(std::time::Instant::now());
            patch
        };
        self.store
            .update_foreman_inflight(self.row_id, &patch)
            .await
    }

    /// **收口**（spec 决策 1）：把半截行写成完整行，`status` 落 `NULL`。
    pub(super) async fn close(&self, msg: NewForemanMessage) -> Result<i64> {
        let seq = {
            let mut st = self.state.lock().unwrap();
            st.finished = true;
            // 收口时全部段序都已随终值落库，预留的位置号在这里一并算数。
            st.seq.max(st.reserved)
        };
        self.unregister();
        self.store
            .close_foreman_inflight(self.row_id, msg, seq)
            .await
    }

    /// **丢弃**：这一轮在本进程里没跑起来 / 不落回话行——今天的语义是库里没有它那一行
    /// （决策 211④ 的失败账另有 `system` 行承载），故半截行跟着一起消失，不留一条永远
    /// 「正在说」的空壳。失败只记日志：丢弃本身不该把这一轮的结局改成失败。
    pub(super) async fn discard(&self, why: &'static str) {
        {
            let mut st = self.state.lock().unwrap();
            if st.finished {
                return;
            }
            st.finished = true;
        }
        self.unregister();
        if let Err(e) = self.store.discard_foreman_inflight(self.row_id).await {
            tracing::warn!(row_id = self.row_id, why, error = %e, "丢弃在途半截行失败");
        }
    }

    /// 摘登记（只摘自己那一条：同班次被后来者顶掉时，别人家的入口不该被我带走）。
    fn unregister(&self) {
        let mut live = FOREMAN_LIVE_TURNS.lock().unwrap();
        if live
            .get(&self.session_id)
            .is_some_and(|t| t.row_id == self.row_id)
        {
            live.remove(&self.session_id);
        }
    }
}

impl Drop for LiveTurn {
    fn drop(&mut self) {
        let finished = match self.state.lock() {
            Ok(st) => st.finished,
            Err(e) => e.into_inner().finished,
        };
        if finished {
            return;
        }
        // 没走到收口也没走到丢弃（`?` 早退 / panic）：尽力把半截行摘掉——同上，
        // 今天这一轮在库里不该有行。摘不掉（运行时已退）时行留在库里，由票 03 的
        // 启动恢复标成「已中断」——进程被杀那条路走不到 Drop，靠的正是那一步。
        self.unregister();
        let store = self.store.clone();
        let row_id = self.row_id;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = store.discard_foreman_inflight(row_id).await {
                    tracing::warn!(row_id, error = %e, "清理未收口的在途半截行失败");
                }
            });
        }
    }
}

/// 流式增量的入口（provider 的 `emit_delta` 调）：按班次找到在飞现场并推进它。
///
/// **没登记就是零成本的一次锁 + 空查**：流水线节点的 `session_id` 恒为空串，
/// 连查表都到不了。
pub fn push_foreman_live_delta(
    session_id: &str,
    channel: Channel,
    text: &str,
) -> Option<(i64, u64)> {
    if session_id.is_empty() || text.is_empty() {
        return None;
    }
    let turn = FOREMAN_LIVE_TURNS
        .lock()
        .unwrap()
        .get(session_id)
        .cloned()?;
    turn.push_delta(channel, text)
}

/// 节流刷库的入口（provider 的流式循环每收一块调一次）。
///
/// 节拍判据在现场里（`last_flush` / `dirty`），这里只负责取到现场——取不到就什么都不做：
/// 这一轮已经收口，迟到的块没有地方可落（收口写是终态）。
pub async fn flush_foreman_live_turn(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let Some(turn) = FOREMAN_LIVE_TURNS.lock().unwrap().get(session_id).cloned() else {
        return;
    };
    if let Err(e) = turn.flush(false).await {
        tracing::warn!(session_id, error = %e, "在途半截行的节流刷写失败");
    }
}

/// 人的那一轮**单独**的在飞计数（决策 289 / 票 03）：`say` 起、`say` 落；值守轮不置它。
///
/// 为什么不读 [`FOREMAN_TURNS`]：那格答的是「这一班有没有一轮在跑」，两条时间线分家之后
/// 各自登记各自的班次——值守轮在另一个 session_id 上，从那里看不出人正在说话。而裁决 2
/// 的排队判据要的恰是「**人**在不在跑」：同一时刻两份十几万 token 的上下文打同一个
/// provider（2026-09-26 实测并行 7.8 分钟），既是浪费也拖慢人的那一轮。
///
/// 计数挂在 **runner 实例**上而不是全局 static：生产里 `AppState` 只有一个值班长，
/// 实例级与进程级是同一个读数；全局 static 则会让「人在跑」这个现场漏进任何一段
/// 无关的并发代码——测试（并跑的用例各建各的 runner）是它第一个咬到的地方。
#[derive(Default)]
pub(super) struct HumanTurns(Arc<AtomicUsize>);

/// [`HumanTurns`] 的登记凭据：退出即摘（与 [`ForemanTurnGuard`] 同一姿态）。
pub struct HumanTurnGuard(Arc<AtomicUsize>);

impl Drop for HumanTurnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl HumanTurns {
    pub(super) fn begin(&self) -> HumanTurnGuard {
        self.0.fetch_add(1, Ordering::SeqCst);
        HumanTurnGuard(Arc::clone(&self.0))
    }

    pub(super) fn in_flight(&self) -> bool {
        self.0.load(Ordering::SeqCst) > 0
    }
}

/// 人这一轮的**停钮**通道（决策 294 / 票 09）：一次「请求停下」的观察点。
///
/// 形状照流水线那套（[`crate::pipeline::executor`] 的 `CancelSignal` / `EXECUTOR_REGISTRY`，
/// 决策 226 / 276），只有一条来路（人按停，值守轮归开关——裁决 10），故没有 origin 那一格：
/// 用 `AtomicBool` + `Notify` 而不是单一个 `Notify` 的理由与那边一字不差——**两条路都要走通**，
/// 轮停在模型调用上时靠 `wait()` 把它唤醒，信号先于观察者到达（或它正走在两轮之间）时靠
/// 那个布尔值在下一轮开头拦住它。只用 `notify_waiters()` 会丢信号，只用布尔值则要等到
/// 下一轮开头——而停住的恰恰就是那一轮。
#[derive(Clone, Default)]
pub(super) struct TurnCancel {
    requested: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl TurnCancel {
    pub(super) fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        // `notify_one` 而非 `notify_waiters`：无人等待时它**存一个许可**，
        // 于是「信号先到、观察者后建」这个窗口也不会丢。
        self.notify.notify_one();
    }

    pub(super) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub(super) async fn wait(&self) {
        self.notify.notified().await;
    }
}

/// 会话 → 在飞那个人这一轮的停钮通道（进程内，照 `EXECUTOR_REGISTRY` 的先例）。
///
/// **为什么挂在班次上而不是 runner 实例上**：停钮的发出方是**另一个 HTTP 请求**
/// （`POST /foreman/sessions/{id}/cancel`），它只拿得到班次 id——实例级的登记（`HumanTurns`
/// 那格）对它是不可见的。跨进程的情形与 `FOREMAN_TURNS` 同一姿态：那一个实例里的在飞轮
/// 本来就随它的进程一起没了，本进程查不到登记就是「没有在跑」。
static FOREMAN_TURN_CANCELS: LazyLock<Mutex<HashMap<String, TurnCancel>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 请求停掉 `session_id` 上正在跑的那个人这一轮，返回「当时确实有一轮在跑」。
///
/// `false` 的两种来路都要如实带上：① 还没有人在说话（或已经说完了）；② 那一轮是**值守轮**
/// ——自动轮不设停钮（裁决 10：值守轮归开关），它的通道根本不登记。
pub fn cancel_foreman_turn(session_id: &str) -> bool {
    match FOREMAN_TURN_CANCELS.lock().unwrap().get(session_id) {
        Some(signal) => {
            signal.request();
            true
        }
        None => false,
    }
}

/// 人这一轮停钮通道的登记凭据：退出即摘（与 [`ForemanTurnGuard`] / [`HumanTurnGuard`] 同一姿态）。
///
/// 只摘**自己那一格**：同一班理论上可以并排起两轮（界面拦着，但服务端不假定），后来的
/// 那一轮会覆盖前一轮的槽——按指针比一下才知道这格是不是自己的，否则先退出的那一轮会把
/// 另一轮的停钮通道顺手摘掉（执行体那边用「代次」表达同一件事，见 `RegistryEntry`）。
pub(super) struct TurnCancelGuard {
    session_id: String,
    signal: TurnCancel,
}

impl TurnCancelGuard {
    pub(super) fn signal(&self) -> &TurnCancel {
        &self.signal
    }
}

impl Drop for TurnCancelGuard {
    fn drop(&mut self) {
        let mut registry = FOREMAN_TURN_CANCELS.lock().unwrap();
        let mine = registry
            .get(&self.session_id)
            .is_some_and(|s| Arc::ptr_eq(&s.notify, &self.signal.notify));
        if mine {
            registry.remove(&self.session_id);
        }
    }
}

/// 登记这一班的人这一轮，返回退出即自动摘的凭据（决策 294 / 票 09）。
pub(super) fn begin_turn_cancel(session_id: &str) -> TurnCancelGuard {
    let signal = TurnCancel::default();
    FOREMAN_TURN_CANCELS
        .lock()
        .unwrap()
        .insert(session_id.to_string(), signal.clone());
    TurnCancelGuard {
        session_id: session_id.to_string(),
        signal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_TURNS: LazyLock<TurnRegistry> = LazyLock::new(|| TurnRegistry {
        counts: Mutex::new(HashMap::new()),
    });

    /// bool 按格覆盖的错法（决策 260 的原始动机）钉在单测里：同班两轮并跑，
    /// 先退出的那一轮不得把另一轮的登记一起摘掉。
    #[test]
    fn dropping_one_of_two_concurrent_turns_keeps_the_session_in_flight() {
        let g1 = TEST_TURNS.begin("t-concurrent");
        let g2 = TEST_TURNS.begin("t-concurrent");
        assert!(TEST_TURNS.in_flight("t-concurrent"));
        drop(g1);
        assert!(
            TEST_TURNS.in_flight("t-concurrent"),
            "先退出的一轮不得摘掉另一轮的登记（bool 按格覆盖的错法）"
        );
        drop(g2);
        assert!(!TEST_TURNS.in_flight("t-concurrent"));
    }

    /// 登记按会话分格：一班的凭据动不了另一班的读数。
    #[test]
    fn sessions_are_independent_keys() {
        let ga = TEST_TURNS.begin("t-key-a");
        let _gb = TEST_TURNS.begin("t-key-b");
        drop(ga);
        assert!(!TEST_TURNS.in_flight("t-key-a"));
        assert!(TEST_TURNS.in_flight("t-key-b"));
    }

    /// 生产接线走同一份类型接口：`begin_foreman_turn` / `foreman_turn_in_flight`
    /// 是 [`TurnRegistry`] 的两个薄壳，不是第二套记账。
    #[test]
    fn production_wiring_roundtrips_through_turn_registry() {
        let session = "t-production-wiring";
        assert!(!foreman_turn_in_flight(session));
        let guard = begin_foreman_turn(session);
        assert!(foreman_turn_in_flight(session));
        drop(guard);
        assert!(!foreman_turn_in_flight(session));
    }
}
