//! 值班长会话存储（决策 176 / 182 / 204，票 01 / 05）。
//!
//! **独立于 `kanban_node_runs` / `kanban_node_conversations`**：那两张表的归属是
//! 「任务 / 项目恰好一个非空」（迁移 0004 的 CHECK），而值班长是**本机夜班级**的角色
//! ——没有任务，首启时也可以没有项目。放宽那条 CHECK 会让阶段聚合混进无阶段行，
//! 代价大于收益（决策 182）。理由与取舍写在迁移 0005 的头部。
//!
//! **会话（班次）是对话的容器**（决策 204）：一条长台账拆成一排可以新建 / 切换 / 重命名 /
//! 归档的班次。隔离的是**上下文**——喂给模型的 transcript 与页头的 token 合计都按会话过滤；
//! **不隔离权限**，也不隔离态势快照（`read_task` / `read_conversation` 与 `build_briefing`
//! 照旧全局。「换会话 ≠ 换看板」）。
//!
//! **不进 `total_tokens` / `total_calls`**：那两个字段的口径是「流水线执行 + 伪阶段」。
//! 对话自身的 token 由本表承载，对讲台据此自报「本次会话 N tok」——按会话过滤之后，
//! 那个数字第一次名副其实（决策 204⑤：此前它是对整张表求和，即「自建库以来的累计值」）。

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;

use super::{parse_ts, ts, Store};
use crate::Result;

/// `role` 的三个取值（与迁移 0005 的列注释同源）。
pub const FOREMAN_ROLE_USER: &str = "user";
pub const FOREMAN_ROLE_ASSISTANT: &str = "assistant";
/// 操作台自己记的一轮（决策 207）：提议的执行结果。
///
/// **不能塞进 assistant**：那一侧是「值班长说的话」，而提议的结果恰恰是它**做不了**
/// 的那件事的下场。写成 assistant，模型下一轮读到自己的历史时会把这段当成自己说过的话
/// ——「你已经写入了 notes.md」这种声称正是人格里第一条纪律要挡的东西。
pub const FOREMAN_ROLE_SYSTEM: &str = "system";

/// 会话标题的字符上限（决策 204②：「前若干字」落到这个数）。
///
/// 24 是窄屏 chip 上还能认出这一班是干什么的长度——再长就被 CSS 截断了，
/// 而截断发生在渲染层意味着库里的标题与看到的标题不是一个东西。
pub const SESSION_TITLE_MAX_CHARS: usize = 24;

/// 还没说出第一句话的会话叫这个（决策 204②「没有首条用户消息时给一个中性标题」）。
pub const FOREMAN_SESSION_DEFAULT_TITLE: &str = "新班次";

/// 会话的两种身份（决策 286 / 票 foreman-unbounded 01）：人的班次与值守台账。
///
/// 同一套表、同一批读路径，靠这一列分家——「分家」分的是**数据归属**（值守轮写的话
/// 落它自己的班次），不是权限，也不是工具面（裁决 4：值守轮能查什么一字不改）。
pub const FOREMAN_SESSION_KIND_TALK: &str = "talk";
pub const FOREMAN_SESSION_KIND_WATCH: &str = "watch";

/// 值守班次的固定标题（决策 286 / 票 01）。它不是给人起的名字，是「这一本是台账」的标识
/// ——对讲台的值守入口按 `kind` 取它，标题只是列表里那行字。
pub const FOREMAN_WATCH_SESSION_TITLE: &str = "值守台账";

/// `status` 的两个取值（迁移 0036；`NULL` = 已收口的正常行，与 `role` 同一条
/// 「观测字段不兜底」的口径）。
///
/// **在途**（票 01）：一轮正在跑，这一行是它当场建的半截行，随广播节流刷写；收口时
/// 同一行被写成完整行（`status` 落 `NULL`）——对讲台重进 refetch 就看得到前半段。
/// **已中断**（票 03）：进程被杀留下的半截行，由启动恢复标上，半截轮从此有痕迹。
pub const FOREMAN_MESSAGE_IN_FLIGHT: &str = "in_flight";
pub const FOREMAN_MESSAGE_INTERRUPTED: &str = "interrupted";

/// 一次列出的会话上限（不分页，照 `SESSION_PAGE_LIMIT` 的精神）。
///
/// 班次是**按天/按班**的粒度，不是按调用——一个人一晚上不会新建 50 个班次。
/// 真撞上这个上限时，说明有人把它当成了日志列表用，而那件事该由台账去回答。
pub const FOREMAN_SESSION_LIST_LIMIT: usize = 50;

/// 会话标题：首条用户消息 → 一行标题（决策 204②）。
///
/// **纯函数**：迁移 0012 的回填标题在 SQL 里写了同一条规则（`TRIM` → `SUBSTR(...,1,24)`
/// → 超长补省略号），两处必须给出同一个答案——SQLite 的 `SUBSTR` / `LENGTH` 对 TEXT
/// 计数的是字符而不是字节，与这里 `.chars()` 的语义相同。改这里就要改那条 SQL。
pub fn session_title_from(first_message: Option<&str>) -> String {
    let trimmed = first_message.unwrap_or("").trim();
    if trimmed.is_empty() {
        return FOREMAN_SESSION_DEFAULT_TITLE.to_string();
    }
    let mut title: String = trimmed.chars().take(SESSION_TITLE_MAX_CHARS).collect();
    if trimmed.chars().count() > SESSION_TITLE_MAX_CHARS {
        title.push('…');
    }
    title
}

/// 一行会话（班次）。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanSession {
    pub id: String,
    pub title: String,
    /// 班次身份（决策 286 / 票 01）：[`FOREMAN_SESSION_KIND_TALK`] 或
    /// [`FOREMAN_SESSION_KIND_WATCH`]。迁移 0030 起带 CHECK，库里的值只可能是这两个；
    /// 认不出的值原样带出去（与 `role` 同一条「观测字段不兜底」的口径）。
    pub kind: String,
    pub created_at: DateTime<Utc>,
    /// 最近一次说话的时间。列表按它倒序（决策 204⑦）。
    pub last_active_at: DateTime<Utc>,
    /// 归档时间。归档 = 从列表里收起来，**不物理删除**，也不保护消息（决策 204⑦）。
    pub archived_at: Option<DateTime<Utc>>,
}

/// 一行值班长会话。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanMessage {
    pub id: i64,
    /// 所属会话。会话隔离上下文，故它是所有读取的**必填**入口参数，不是可选项。
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 该轮注入的夜班态势快照（审计：它当时看到的是这份读数）。用户消息为 `None`。
    pub briefing_json: Option<Value>,
    /// 该轮调用过的只读工具痕迹（票 05）。无工具调用时为 `None`。
    pub traces_json: Option<Value>,
    /// 该轮**按发生顺序**记下的步骤序列（决策 273）。
    ///
    /// 与 `thinking` / `traces_json` / `content` 是同一件事的两种看法：那三列各自只剩一类
    /// 东西（推理拼成一段、工具合成一张表、收口那句一段），**顺序丢了**；这一列保留
    /// 「先想 → 再查 → 然后说」的原样。无任何一步时为 `None`。
    pub segments_json: Option<Value>,
    /// 本轮**机器读出的改动文件清单**（票 01）：`edit_file` / `write_file` 的路径参数
    /// （去重、保首次出现序）；本轮若调过 `repair(finish)`，再并上那份权威 diff 的文件。
    ///
    /// 两个消费者：① 收口正文末尾那一行【本轮改动】由它渲染；② **下一轮**——这一轮的台账行
    /// 是值班长认识「我做过什么」的唯一读物（`content` 进 prompt，本列随行一并带去）。
    /// `None` = 这一轮没动过文件（与 `traces_json` 同一条口径：「没有」与「有但是空的」是两件事）。
    pub changed_files_json: Option<Value>,
    /// 该轮的**推理 / 思考**原文（决策 244）。不产推理的模型为 `None`。
    ///
    /// **展示留痕，永不回灌**：它不是 assistant 消息的一部分，进 `transcript` 既会被
    /// 部分厂商拒绝，也会让每一轮白烧一份最长的文本。
    pub thinking: Option<String>,
    /// 结构化选项提问的载荷（决策 265）：`{"question": …, "options": [2–4 句]}`。
    ///
    /// 只在**值班长发问的那一轮** assistant 行上在场；用户 / system 行恒 `None`
    /// （用户行的 INSERT 不写这一列——问话只有工具执行点那一个生产者）。
    pub ask_json: Option<Value>,
    /// 行的**在途状态**（迁移 0036，票 01）：`None` = 已收口的正常行（绝大多数），
    /// [`FOREMAN_MESSAGE_IN_FLIGHT`] = 一轮正在跑的半截行，[`FOREMAN_MESSAGE_INTERRUPTED`]
    /// = 进程被杀留下的半截行（票 03）。取值域见那两个常量的说明。
    pub status: Option<String>,
    /// **行内位置序号**（迁移 0036，票 02）：在途事件的去重基准。只对 `in_flight` 行有
    /// 意义——前端取快照记 `seq0`，只接 `seq > seq0` 的增量（**seq 只做去重，不做回放**，
    /// 决策 275 不变）。存量行与收口行恒 0。
    pub seq: i64,
    /// **中断时刻**（迁移 0036，票 03）：`status = 'interrupted'` 的行记下什么时候断的，
    /// 时间线据此把「已中断」与断的那一刻一起摆出来。其余行恒 `None`。
    pub interrupted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 待写入的一行（`id` / `created_at` 由存储层给，不接调用方的时钟）。
#[derive(Debug, Clone)]
pub struct NewForemanMessage {
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing_json: Option<Value>,
    pub traces_json: Option<Value>,
    /// 顺序留痕（决策 273，见 [`ForemanMessage::segments_json`]）。便捷路径都给 `None`——
    /// 只有值班长那一轮会填它，且由 [`crate::pipeline::foreman`] 直接赋值。
    pub segments_json: Option<Value>,
    /// 本轮机器读出的改动文件清单（票 01，见 [`ForemanMessage::changed_files_json`]）。
    /// 便捷路径都给 `None`——填它的只有值班长收口处与 repair 那一轮。
    pub changed_files_json: Option<Value>,
    /// 该轮的推理原文（决策 244）。构造它的两条便捷路径都给 `None`——
    /// 只有值班长那一轮会填它，且由 [`crate::pipeline::foreman`] 直接赋值。
    pub thinking: Option<String>,
    /// 结构化选项提问的载荷（决策 265）。便捷路径同样给 `None`——填它的只有
    /// `respond_inner`（槽收口处，每轮至多一个）。
    pub ask_json: Option<Value>,
}

impl NewForemanMessage {
    /// 操作台记的一轮（决策 207）：提议执行 / 拒绝的结果，或一次失败的理由。
    pub fn system(session_id: impl Into<String>, content: impl Into<String>) -> Self {
        NewForemanMessage {
            session_id: session_id.into(),
            role: FOREMAN_ROLE_SYSTEM.to_string(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            // 操作台的账不代表「一轮的产出」——它记的是别人按下之后的结果。
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        }
    }

    /// 值班长回的话：带读数与（可能的）工具痕迹。
    pub fn assistant(session_id: impl Into<String>, content: impl Into<String>) -> Self {
        NewForemanMessage {
            session_id: session_id.into(),
            role: FOREMAN_ROLE_ASSISTANT.to_string(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            // 便捷路径不带痕迹，也就无从读出改动清单；填它的只有收口处。
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        }
    }
}

/// 在途半截行的**节流批写**载荷（票 01，spec 决策 1 / 3）。
///
/// 「这一轮说到哪了」的中途读数：[`Store::update_foreman_inflight`] 一次把这几列整体
/// 写上去——`respond_inner` 在每次模型调用的边界上给权威值（段序 / 痕迹 / 累计推理 /
/// token），provider 在流式途中推逐字正文与推理增量；收口时被最终值整体替换，
/// 中途这份只是过程态。
#[derive(Debug, Clone, Default)]
pub struct InFlightPatch {
    /// 正在往外冒的那段正文（收口那句的半成品）。收口时换成权威 `content`。
    pub content: String,
    /// 累计推理（权威值整体写，不增量拼——拼会把跨调用的段落粘连）。
    pub thinking: Option<String>,
    /// 段序（决策 273）：已收场的步骤。`None` = 一步都没有。
    pub segments_json: Option<Value>,
    /// 工具痕迹聚合（同 `NewForemanMessage::traces_json`）。
    pub traces_json: Option<Value>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// **已覆盖到的位置**（票 02）：写进 `seq` 列，成为前端拼接的 `seq0`。只写**已进
    /// 现场**的那些位置——广播了但还没随这次刷写落库的（在途的工具事件）不计，
    /// 否则快照会声称覆盖了它其实没有的字（接缝于是丢字）。
    pub seq: u64,
}

#[derive(FromRow)]
struct ForemanSessionRow {
    id: String,
    title: String,
    kind: String,
    created_at: String,
    last_active_at: String,
    archived_at: Option<String>,
}

impl ForemanSessionRow {
    fn into_session(self) -> Result<ForemanSession> {
        Ok(ForemanSession {
            id: self.id,
            title: self.title,
            kind: self.kind,
            created_at: parse_ts(&self.created_at)?,
            last_active_at: parse_ts(&self.last_active_at)?,
            archived_at: self.archived_at.as_deref().map(parse_ts).transpose()?,
        })
    }
}

#[derive(FromRow)]
struct ForemanMessageRow {
    id: i64,
    session_id: String,
    role: String,
    content: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    briefing_json: Option<String>,
    traces_json: Option<String>,
    segments_json: Option<String>,
    changed_files_json: Option<String>,
    thinking: Option<String>,
    ask_json: Option<String>,
    status: Option<String>,
    seq: i64,
    interrupted_at: Option<String>,
    created_at: String,
}

impl ForemanMessageRow {
    fn into_message(self) -> Result<ForemanMessage> {
        Ok(ForemanMessage {
            id: self.id,
            session_id: self.session_id,
            // role 是观测类字段：非法值不该打垮整个查询（storage/mod.rs 的 Q6 分类），
            // 但它同时决定界面把这句话摆在谁那一侧，故不兜底成别的角色——
            // 原样带出去，前端按 `=== "user"` 判定，不认识的值落到「值班长」一侧。
            role: self.role,
            content: self.content,
            prompt_tokens: self.prompt_tokens.max(0) as u32,
            completion_tokens: self.completion_tokens.max(0) as u32,
            briefing_json: self.briefing_json.as_deref().map(parse_json).transpose()?,
            traces_json: self.traces_json.as_deref().map(parse_json).transpose()?,
            segments_json: self.segments_json.as_deref().map(parse_json).transpose()?,
            changed_files_json: self
                .changed_files_json
                .as_deref()
                .map(parse_json)
                .transpose()?,
            thinking: self.thinking,
            ask_json: self.ask_json.as_deref().map(parse_json).transpose()?,
            status: self.status,
            seq: self.seq.max(0),
            interrupted_at: self.interrupted_at.as_deref().map(parse_ts).transpose()?,
            created_at: parse_ts(&self.created_at)?,
        })
    }
}

/// 审计 JSON 列解析。损坏即报错——它与 `pending_reason` 同类：决定「依据什么」这一
/// 审计结论，静默兜底会让页面显示一份从未存在过的快照。
fn parse_json(raw: &str) -> Result<Value> {
    Ok(serde_json::from_str(raw)?)
}

const FOREMAN_MESSAGE_COLUMNS: &str = "id, session_id, role, content, prompt_tokens, \
                                       completion_tokens, briefing_json, traces_json, \
                                       segments_json, changed_files_json, thinking, ask_json, \
                                       status, seq, interrupted_at, created_at";

const FOREMAN_SESSION_COLUMNS: &str = "id, title, kind, created_at, last_active_at, archived_at";

impl Store {
    /// 值班长这一轮的**落库状态读数**（决策 310 判据③）：提议数 / 任务状态 / 游标位置。
    ///
    /// 三样都是「有没有推动台账」的**直读**：`StateReadings::digest()` 一比就知道变了没有。
    /// 取明细而不是聚合计数，是因为排障时要能直接看到「哪一行动了」，而明细拼成一行再哈希
    /// 的成本与计数没有区别。
    ///
    /// 与 [`Store::count_round_foreman_proposals`] 同一把「这一轮」的尺（`created_at >= since`）。
    pub async fn foreman_state_readings(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<crate::agent::loops::StateReadings> {
        let proposals = self
            .count_round_foreman_proposals(session_id, since)
            .await?;
        let tasks: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, status FROM kanban_tasks WHERE archived_at IS NULL ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        let cursors: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT cursor_id, status, stage, node FROM kanban_node_cursors
             WHERE status != 'archived' ORDER BY cursor_id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(crate::agent::loops::StateReadings {
            proposals,
            tasks: tasks
                .into_iter()
                .map(|(id, status)| format!("{id}:{status}"))
                .collect::<Vec<_>>()
                .join(","),
            cursors: cursors
                .into_iter()
                .map(|(id, status, stage, node)| format!("{id}:{status}:{stage}:{node}"))
                .collect::<Vec<_>>()
                .join(","),
        })
    }

    // ─────────────────────────── 会话（班次）───────────────────────────

    /// 会话列表，按最近活动倒序、上限一条常量、不分页（决策 204⑦）。
    ///
    /// `kind` 过滤（决策 286 / 票 01）：`Some("talk")` / `Some("watch")` 只回那一类，
    /// `None` 回全部。路由层缺省传 talk——「对讲台的班次列表」与「值守台账的入口」
    /// 是两个列表；混合回一份会让前端把值守台账当成一个可说话的班次。
    ///
    /// 归档的**缺省不在这个列表里**——「从列表里收起来」就是归档的全部含义；给出
    /// `include_archived` 时它们照常在（票 06：chip 行的「显示已归档」开关），灰不灰
    /// 由界面按 `archived_at` 判。归档行本就仍能按 id 单独取到（`get_foreman_session`）。
    pub async fn list_foreman_sessions(
        &self,
        kind: Option<&str>,
        include_archived: bool,
    ) -> Result<Vec<ForemanSession>> {
        // WHERE 子句按开关拼：两条路共用同一条 SQL 与同一组绑定，缺省语义不会漂。
        let mut wheres: Vec<&str> = Vec::new();
        if !include_archived {
            wheres.push("archived_at IS NULL");
        }
        if kind.is_some() {
            wheres.push("kind = ?");
        }
        let where_sql = if wheres.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", wheres.join(" AND "))
        };
        let sql = format!(
            "SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions
             {where_sql}
             ORDER BY last_active_at DESC, id DESC LIMIT ?"
        );
        // 占位符按出现顺序绑定：kind 过滤在前、LIMIT 在后。
        let query = sqlx::query_as::<_, ForemanSessionRow>(&sql);
        let query = match kind {
            Some(k) => query.bind(k),
            None => query,
        };
        let rows: Vec<ForemanSessionRow> = query
            .bind(FOREMAN_SESSION_LIST_LIMIT as i64)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanSessionRow::into_session)
            .collect()
    }

    /// 按 id 取一个会话（含已归档的）。
    pub async fn get_foreman_session(&self, session_id: &str) -> Result<Option<ForemanSession>> {
        let sql =
            format!("SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions WHERE id = ?");
        let row: Option<ForemanSessionRow> = sqlx::query_as(&sql)
            .bind(session_id)
            .fetch_optional(self.pool())
            .await?;
        row.map(ForemanSessionRow::into_session).transpose()
    }

    /// 最近活动的未归档会话，**限定某一类**（决策 286 / 票 01）。
    ///
    /// 人的班次与值守台账各取各的「最近」：值守轮往 watch 班次落账会刷新它的
    /// `last_active_at`，若不限定类别，人的那一轮就会落进值守台账（或反过来）。
    pub async fn latest_foreman_session_of_kind(
        &self,
        kind: &str,
    ) -> Result<Option<ForemanSession>> {
        let sql = format!(
            "SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions
             WHERE archived_at IS NULL AND kind = ?
             ORDER BY last_active_at DESC, id DESC LIMIT 1"
        );
        let row: Option<ForemanSessionRow> = sqlx::query_as(&sql)
            .bind(kind)
            .fetch_optional(self.pool())
            .await?;
        row.map(ForemanSessionRow::into_session).transpose()
    }

    /// 最近活动的未归档**人的**班次（对讲台的默认落点）。
    ///
    /// 现有调用者（`say` 的缺省落点、读端点的缺省班次、中断账的归属）说的全是这一类
    /// ——值守台账不该被任何一条「缺省落到最近班次」的老路挑中。
    pub async fn latest_foreman_session(&self) -> Result<Option<ForemanSession>> {
        self.latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_TALK)
            .await
    }

    /// 新建一个**人的**班次。`title` 为空（或全空白）时用中性标题。
    pub async fn create_foreman_session(&self, title: &str) -> Result<ForemanSession> {
        self.create_foreman_session_of_kind(FOREMAN_SESSION_KIND_TALK, title)
            .await
    }

    /// 新建一个**指定身份**的班次（决策 286 / 票 01）。
    ///
    /// 值守班次由值守轮按需自建（固定标题[`FOREMAN_WATCH_SESSION_TITLE`]），不经
    /// `POST /foreman/sessions`——那条路只开人的班次，这是「值守台账不是聊天室」
    /// 在写入面的形状。
    pub async fn create_foreman_session_of_kind(
        &self,
        kind: &str,
        title: &str,
    ) -> Result<ForemanSession> {
        let now = self.now();
        let id = ulid::Ulid::new().to_string();
        let title = if title.trim().is_empty() {
            FOREMAN_SESSION_DEFAULT_TITLE.to_string()
        } else {
            title.trim().to_string()
        };
        sqlx::query(
            "INSERT INTO kanban_foreman_sessions (id, title, kind, created_at, last_active_at, archived_at)
             VALUES (?, ?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&title)
        .bind(kind)
        .bind(ts(now))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        Ok(ForemanSession {
            id,
            title,
            kind: kind.to_string(),
            created_at: now,
            last_active_at: now,
            archived_at: None,
        })
    }

    /// 改名。返回改后的会话；会话不存在时 `None`（路由据此回 404）。
    ///
    /// **已归档的会话也能改名**：归档不是封存，恢复要靠改名找回来这条不成立，
    /// 但拒绝改名也没有任何好处——两边都不解决「怎么把归档的弄回来」。
    pub async fn rename_foreman_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<ForemanSession>> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(None);
        }
        let affected = sqlx::query("UPDATE kanban_foreman_sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(session_id)
            .execute(self.pool())
            .await?
            .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get_foreman_session(session_id).await
    }

    /// 归档（置 `archived_at`）。幂等：重复归档不改第一次的时间戳。
    pub async fn archive_foreman_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ForemanSession>> {
        sqlx::query(
            "UPDATE kanban_foreman_sessions SET archived_at = ?
             WHERE id = ? AND archived_at IS NULL",
        )
        .bind(ts(self.now()))
        .bind(session_id)
        .execute(self.pool())
        .await?;
        self.get_foreman_session(session_id).await
    }

    // ─────────────────────────── 会话行 ───────────────────────────

    /// 追加一行会话，返回其 id。同事务刷新所属会话的 `last_active_at`。
    pub async fn append_foreman_message(&self, msg: NewForemanMessage) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (session_id, role, content, prompt_tokens, completion_tokens, briefing_json,
              traces_json, segments_json, changed_files_json, thinking, ask_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&msg.session_id)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(msg.prompt_tokens as i64)
        .bind(msg.completion_tokens as i64)
        .bind(msg.briefing_json.as_ref().map(Value::to_string))
        .bind(msg.traces_json.as_ref().map(Value::to_string))
        .bind(msg.segments_json.as_ref().map(Value::to_string))
        .bind(msg.changed_files_json.as_ref().map(Value::to_string))
        .bind(msg.thinking.as_deref())
        .bind(msg.ask_json.as_ref().map(Value::to_string))
        .bind(ts(now))
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE kanban_foreman_sessions SET last_active_at = ? WHERE id = ?")
            .bind(ts(now))
            .bind(&msg.session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// 一轮开工即建的**在途 assistant 半截行**（票 01，spec 决策 1），返回行 id。
    ///
    /// `content` 起手为空、三份聚合列（`briefing_json` 之外）都还是 `NULL`——「还没说过」
    /// 与「说了一个空」是两件事（与 `traces_json` 的「没有 ≠ 有但是空的」同一条口径）。
    /// `briefing_json` 一上来就写：快照是这一轮的**输入**，从开工那一刻就成立，审计要的
    /// 正是「它当时看到的是这份读数」。
    ///
    /// 不刷 `last_active_at`：会话的「最近说话」由真落了话的那次刷新（收口那一下），
    /// 在途只是把这一轮的现场建出来——建了但没跑完的轮（丢弃 / 中断）不该把班次顶上去。
    pub async fn begin_foreman_inflight(
        &self,
        session_id: &str,
        briefing_json: Option<Value>,
    ) -> Result<i64> {
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (session_id, role, content, prompt_tokens, completion_tokens, briefing_json,
              status, created_at)
             VALUES (?, ?, '', 0, 0, ?, ?, ?) RETURNING id",
        )
        .bind(session_id)
        .bind(FOREMAN_ROLE_ASSISTANT)
        .bind(briefing_json.as_ref().map(Value::to_string))
        .bind(FOREMAN_MESSAGE_IN_FLIGHT)
        .bind(ts(now))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    /// 在途行的**节流批写**（票 01，spec 决策 1：合批节流的节拍是实现细节）。
    ///
    /// `WHERE status = 'in_flight'` 是这道口子的全部纪律：收口 / 丢弃之后这条 UPDATE 是
    /// 空操作，**迟到的刷写改不动终态**——中途刷写与收口写之间不存在「后写的覆盖掉完整行」
    /// 这条竞态（同一轮的写入本就串在同一任务上，这行谓词防的是跨轮的迟到者）。
    pub async fn update_foreman_inflight(&self, row_id: i64, patch: &InFlightPatch) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_foreman_messages
             SET content = ?, thinking = ?, segments_json = ?, traces_json = ?,
                 prompt_tokens = ?, completion_tokens = ?, seq = ?
             WHERE id = ? AND status = ?",
        )
        .bind(&patch.content)
        .bind(&patch.thinking)
        .bind(patch.segments_json.as_ref().map(Value::to_string))
        .bind(patch.traces_json.as_ref().map(Value::to_string))
        .bind(patch.prompt_tokens as i64)
        .bind(patch.completion_tokens as i64)
        .bind(patch.seq as i64)
        .bind(row_id)
        .bind(FOREMAN_MESSAGE_IN_FLIGHT)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 收口：把半截行写成**完整行**（`status` → `NULL`），返回行 id。
    ///
    /// 与 [`Self::append_foreman_message`] 同一形状的落库（同一事务刷 `last_active_at`），
    /// 差别只有「写已有的那一行」而不是追加——于是收口后台账仍是「一次回话 = user 行 +
    /// assistant 行」两行，边流边写不改变行数语义，只改变中途是否可见（spec 决策 1）。
    pub async fn close_foreman_inflight(
        &self,
        row_id: i64,
        msg: NewForemanMessage,
        seq: u64,
    ) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let now = self.now();
        sqlx::query(
            "UPDATE kanban_foreman_messages
             SET content = ?, prompt_tokens = ?, completion_tokens = ?, briefing_json = ?,
                 traces_json = ?, segments_json = ?, changed_files_json = ?, thinking = ?,
                 ask_json = ?, seq = ?, status = NULL
             WHERE id = ?",
        )
        .bind(&msg.content)
        .bind(msg.prompt_tokens as i64)
        .bind(msg.completion_tokens as i64)
        .bind(msg.briefing_json.as_ref().map(Value::to_string))
        .bind(msg.traces_json.as_ref().map(Value::to_string))
        .bind(msg.segments_json.as_ref().map(Value::to_string))
        .bind(msg.changed_files_json.as_ref().map(Value::to_string))
        .bind(msg.thinking.as_deref())
        .bind(msg.ask_json.as_ref().map(Value::to_string))
        .bind(seq as i64)
        .bind(row_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE kanban_foreman_sessions SET last_active_at = ? WHERE id = ?")
            .bind(ts(now))
            .bind(&msg.session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row_id)
    }

    /// 丢掉半截行：这一轮在本进程里没跑起来（失败 / 静默不落话），**今天的语义是库里
    /// 没有它那一行**（决策 211④ 的失败账另有 `system` 行承载）——故在途行跟着一起消失，
    /// 不留一条永远「正在说」的空壳。
    ///
    /// `WHERE status = 'in_flight'`：只删在途行。进程被杀留下的那条归票 03 标中断，
    /// 是要**留**的痕迹，不能被这里的清理扫掉。
    pub async fn discard_foreman_inflight(&self, row_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM kanban_foreman_messages WHERE id = ? AND status = ?")
            .bind(row_id)
            .bind(FOREMAN_MESSAGE_IN_FLIGHT)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// **启动恢复：把悬挂的在途行标成已中断**（票 03，与 `orphan_inflight_model_requests`
    /// 同姿势）——**显式修订决策 223**「不做进程退出那一轮的落账」：进程退出的那一轮
    /// 从此落一条**已中断**的半截行，半截轮终于有痕迹（spec .scratch/talk-replay 决策 13）。
    ///
    /// 判据是「库里还挂着 `in_flight`」：本步只在**启动**时跑，此刻进程里没有任何活跃轮
    /// ——还挂着在途的只可能是上一个实例（关掉 / 崩溃）留下的。**只写状态与时刻，不动
    /// 内容**：thinking / 工具步骤原样保留，中断是终态（不提供「接着跑」）。
    /// 时刻走 `now()` 接缝（决策 143），假时钟可测。
    pub async fn mark_orphan_foreman_inflights(&self) -> Result<u64> {
        let now = self.now();
        let marked = sqlx::query(
            "UPDATE kanban_foreman_messages
             SET status = ?, interrupted_at = ?
             WHERE status = ?",
        )
        .bind(FOREMAN_MESSAGE_INTERRUPTED)
        .bind(ts(now))
        .bind(FOREMAN_MESSAGE_IN_FLIGHT)
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(marked)
    }

    /// 追加**值班经理**说的话，并在这是该会话第一句话时按它命名会话（决策 204②）。
    ///
    /// 命名与落库同事务：分两步做的话，中间崩掉会留下一个「有话但还叫新班次」的会话，
    /// 而那个状态没有任何东西会去修它。
    ///
    /// 判据是**该会话有没有过消息**，不是「标题是否等于中性标题」——后者把「人自己
    /// 把会话改名叫『新班次』」当成了「还没命名」，于是下一句话会把人的命名冲掉。
    pub async fn append_foreman_user_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM kanban_foreman_messages WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (session_id, role, content, prompt_tokens, completion_tokens, briefing_json,
              traces_json, thinking, created_at)
             VALUES (?, ?, ?, 0, 0, NULL, NULL, NULL, ?) RETURNING id",
        )
        .bind(session_id)
        .bind(FOREMAN_ROLE_USER)
        .bind(content)
        .bind(ts(now))
        .fetch_one(&mut *tx)
        .await?;
        if before == 0 {
            let title = session_title_from(Some(content));
            sqlx::query(
                "UPDATE kanban_foreman_sessions SET title = ?, last_active_at = ? WHERE id = ?",
            )
            .bind(&title)
            .bind(ts(now))
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("UPDATE kanban_foreman_sessions SET last_active_at = ? WHERE id = ?")
                .bind(ts(now))
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// 某个会话最近 `limit` 条，按 id **升序**返回（时间线顺序，与界面一致）。
    ///
    /// 由新往旧取、再由旧往新排：`LIMIT` 必须作用在最新的那一端，否则一段长对话
    /// 会永远只显示最早几句。`limit = 0` 返回空表（不当作「不限」）。
    ///
    /// **`before_id`：向上游标**（票 05，显式修订后端读接口「不分页」的立场 →
    /// 500 缺省 + 游标补更早段）：给了它就只取**更早的一段**——`id < before_id`
    /// 里最新的 `limit` 条，段内照旧升序；到头返回空表（调用方据此收掉「还有更早」）。
    /// `None` = 缺省那一端（最近 `limit` 条），既有调用一个字都不变。
    pub async fn list_foreman_messages(
        &self,
        session_id: &str,
        limit: usize,
        before_id: Option<i64>,
    ) -> Result<Vec<ForemanMessage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {FOREMAN_MESSAGE_COLUMNS} FROM (
                 SELECT {FOREMAN_MESSAGE_COLUMNS} FROM kanban_foreman_messages
                 WHERE session_id = ? {}
                 ORDER BY id DESC LIMIT ?
             ) ORDER BY id ASC",
            if before_id.is_some() {
                "AND id < ?"
            } else {
                ""
            }
        );
        let mut query = sqlx::query_as::<_, ForemanMessageRow>(&sql).bind(session_id);
        if let Some(before) = before_id {
            query = query.bind(before);
        }
        let rows: Vec<ForemanMessageRow> = query.bind(limit as i64).fetch_all(self.pool()).await?;
        rows.into_iter()
            .map(ForemanMessageRow::into_message)
            .collect()
    }

    /// 本会话合计 `(total_tokens, total_calls)`（决策 204⑤：按会话过滤，不再对整张表求和）。
    ///
    /// `total_calls` 数的是**值班长的回话次数**（assistant 行），不是工具往返次数
    /// ——对讲台要报的是「聊了几轮」，与全局指标的 `total_calls`（run 行数）不是同一个量。
    ///
    /// `status IS NULL` 是「**只算已收口的正常行**」（决策 363③，恢复决策 204⑤「口径不动」）：
    /// 在途行也是 assistant 且带中途刷写的 token 读数，不过滤的话页头「本次会话 N tok」会在
    /// 一轮进行中途跟着涨、失败轮丢弃后再回落——那正是「口径不动」要挡的读数。`status` 的
    /// 取值见 [`FOREMAN_MESSAGE_IN_FLIGHT`] / [`FOREMAN_MESSAGE_INTERRUPTED`]；用户行与收口后
    /// 的 assistant 行都是 `NULL`（收口 SQL 写 `status = NULL`，见 [`Self::close_foreman_inflight`]）。
    pub async fn foreman_session_totals(&self, session_id: &str) -> Result<(u64, u64)> {
        let (tokens, calls): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0),
                    COALESCE(SUM(CASE WHEN role = ? THEN 1 ELSE 0 END), 0)
             FROM kanban_foreman_messages WHERE session_id = ? AND status IS NULL",
        )
        .bind(FOREMAN_ROLE_ASSISTANT)
        .bind(session_id)
        .fetch_one(self.pool())
        .await?;
        Ok((tokens.max(0) as u64, calls.max(0) as u64))
    }

    // 这里**没有** `purge_foreman_messages`：`kanban_foreman_messages` 已退出按
    // `conversation_retention_days` 的年龄清理——对话消息永久保留（票 04，显式修订
    // 决策 182④「同一把保留期尺」与 204⑦「归档不保护消息」，落点见
    // `scheduler::maintenance` 的注释与保留期反向断言那条用例）。谁要再清这张表，
    // 先回去读那两处，别在这里补一个函数了事。
}

// ─────────────────────────── 值守开关（决策 287 / 票 02）───────────────────────────

impl Store {
    /// 值守轮的**全局开关**：行不存在 = 缺省**开**（决策 287 / 票 02）。
    ///
    /// 语义是「关掉跑」：关掉后 `ForemanRunner::watch` 在最前面就返回——有待办也不醒、
    /// 不花钱；在飞的那一轮不受影响（它已经过了这道门）。**单一事实源在库里**：
    /// 值守循环每 10s 问一次（一次单行 SELECT），界面保存后下一趟即生效——
    /// 不需要在进程里再养一份「开关状态」跟库对账。
    pub async fn foreman_watch_enabled(&self) -> Result<bool> {
        let enabled: Option<bool> =
            sqlx::query_scalar("SELECT enabled FROM kanban_foreman_watch WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(enabled.unwrap_or(true))
    }

    /// 写值守开关（UPSERT；行不存在就建）。缺省开的那一格由「行不存在」表达，
    /// 故没有「清除」动作——关掉再打开就落 `true`，与缺省同值不同 provenance
    /// （设置页报「界面保存的」还是「缺省开」）。
    pub async fn set_foreman_watch_enabled(&self, enabled: bool) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_foreman_watch (id, enabled, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
        )
        .bind(enabled)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 界面保存过值守开关吗（provenance 读数）：行存在 = 界面定的。
    pub async fn foreman_watch_has_override(&self) -> Result<bool> {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM kanban_foreman_watch WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(exists.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_is_the_first_message_cut_to_budget() {
        assert_eq!(session_title_from(Some("帮我看下 t1")), "帮我看下 t1");
        assert_eq!(session_title_from(Some("  前后有空白  ")), "前后有空白");
        // 恰好 24 字不补省略号，多一字才补。
        let exactly: String = "字".repeat(SESSION_TITLE_MAX_CHARS);
        assert_eq!(session_title_from(Some(&exactly)), exactly);
        let over: String = "字".repeat(SESSION_TITLE_MAX_CHARS + 1);
        let title = session_title_from(Some(&over));
        assert_eq!(title.chars().count(), SESSION_TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));
        assert_eq!(
            title.chars().filter(|c| *c == '字').count(),
            SESSION_TITLE_MAX_CHARS
        );
    }

    #[test]
    fn session_title_cuts_on_char_boundaries_not_bytes() {
        // 24 个多字节字符不能被切成半个字。
        let text = "流水线卡住了请帮我看一下这个任务到底停在哪一步了谢谢".repeat(2);
        let title = session_title_from(Some(&text));
        let kept: String = text.chars().take(SESSION_TITLE_MAX_CHARS).collect();
        assert_eq!(title, format!("{kept}…"));
    }

    #[test]
    fn empty_first_message_falls_back_to_a_neutral_title() {
        assert_eq!(session_title_from(None), FOREMAN_SESSION_DEFAULT_TITLE);
        assert_eq!(session_title_from(Some("")), FOREMAN_SESSION_DEFAULT_TITLE);
        assert_eq!(
            session_title_from(Some("  \n\t ")),
            FOREMAN_SESSION_DEFAULT_TITLE
        );
    }

    /// 决策 287 / 票 02：行不存在 = 缺省开；写进去读得回来；provenance 随行出现。
    #[tokio::test]
    async fn the_watch_switch_defaults_on_and_persists() {
        let store = Store::open_in_memory(std::sync::Arc::new(crate::clock::SystemClock))
            .await
            .unwrap();
        assert!(store.foreman_watch_enabled().await.unwrap(), "缺省开");
        assert!(!store.foreman_watch_has_override().await.unwrap());

        store.set_foreman_watch_enabled(false).await.unwrap();
        assert!(!store.foreman_watch_enabled().await.unwrap());
        assert!(store.foreman_watch_has_override().await.unwrap());

        store.set_foreman_watch_enabled(true).await.unwrap();
        assert!(store.foreman_watch_enabled().await.unwrap());
        assert!(
            store.foreman_watch_has_override().await.unwrap(),
            "打开也记 provenance：是界面保存的，不是缺省"
        );
    }
}
