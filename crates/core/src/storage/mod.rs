//! SQLite 存储层（决策 13：sqlx migrations；决策 145：测试用临时文件库 + 全量迁移）。
//!
//! 拆分：`tasks`（任务 / 依赖 / 准入）、`cursors`（游标生命周期，执行状态唯一事实来源）、
//! `observability`（runs / 会话 / 命令 / 流转 / 阶段产出）、`catalog`（项目 / provider / 阶段配置）、
//! `foreman`（值班长会话——唯一不挂任务的表，决策 182）、`proposals`（值班长提议——
//! 写动作的落库形态，决策 188 / 207）、`pairing`（配对令牌，票 07）、
//! `server_bind`（界面上的绑定开关，决策 186）、`market_repos`（界面上的技能来源仓名单，
//! 决策 194 继承决策 187 的两级结构）、`skill_sources`（已装技能的来源记录，决策 194）、
//! `notify_channel`（离线通知的界面设置——总开关与通道单元，决策 272⑥⑦⑧）、
//! `attention`（值班长待办——调度器发现的落点，决策 209③）、
//! `model_requests`（模型请求台账——「现在在飞什么」与「这一次烧了多少字节」，决策 231）。

pub mod attention;
pub mod catalog;
pub mod conflict;
pub mod cursors;
pub mod decisions;
pub mod foreman;
pub mod market_repos;
pub mod model_requests;
pub mod notify_channel;
pub mod observability;
pub mod pairing;
pub mod proposals;
pub mod server_bind;
pub mod skill_sources;
pub mod tasks;

pub use attention::{AttentionItem, AttentionKind, WatchWakeOutcome};
pub use foreman::{ForemanMessage, NewForemanMessage};
pub use model_requests::{ModelRequest, ModelRequestStatus, ModelRequestUsage, NewModelRequest};
pub use skill_sources::SkillSource;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::clock::Clock;
use crate::home::Home;
use crate::types::{Node, PendingReason, Stage};
use crate::Result;

/// 数据库句柄。
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    home: Home,
    /// 单次会话落库的最大字符数（§3 `conversation_max_chars`，超出截断）。
    conversation_max_chars: usize,
    /// 离线通知出口（决策 268）：`None` = `[notify].webhook_url` 没配，整段关死。
    /// `Arc<RwLock<..>>` 而不是裸 `RwLock`：`Store` 按值 Clone，裸锁会被一起复制、
    /// 两份状态从此各记各的节流——`Arc` 让所有克隆看见同一个出口与同一份状态。
    notifier: Arc<std::sync::RwLock<Option<Arc<crate::notify::WebhookNotifier>>>>,
}

/// `conversation_max_chars` 的默认值（§3 全局配置表）。
pub const DEFAULT_CONVERSATION_MAX_CHARS: usize = 200_000;

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("home", &self.home.root())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// 打开（必要时创建）家目录下的数据库并跑全量迁移。
    ///
    /// 真实行为优先：WAL + busy_timeout 都可测（决策 145）。
    pub async fn open(home: Home, clock: Arc<dyn Clock>) -> Result<Self> {
        home.ensure_dirs()?;
        let options = SqliteConnectOptions::new()
            .filename(home.db_path())
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Self::migrate(&pool).await?;
        // §12.14：db 与 WAL 伴生文件（可能含明文密钥）一律 0600；目录 0700。
        crate::home::restrict_file_permissions(&home.db_path());
        for side in ["-wal", "-shm"] {
            let p = home.data_dir().join(format!(
                "{}{side}",
                home.db_path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
            if p.exists() {
                crate::home::restrict_file_permissions(&p);
            }
        }
        Ok(Store {
            pool,
            clock,
            home,
            conversation_max_chars: DEFAULT_CONVERSATION_MAX_CHARS,
            notifier: Arc::new(std::sync::RwLock::new(None)),
        })
    }

    /// 内存库：仅用于纯查询逻辑（决策 145）。
    pub async fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Self> {
        let tmp = std::env::temp_dir().join(format!("agentpipeline-mem-{}", ulid::Ulid::new()));
        let home = Home::new(tmp);
        Self::open(home, clock).await
    }

    async fn migrate(pool: &SqlitePool) -> Result<()> {
        sqlx::migrate!("src/storage/migrations").run(pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 开启一个**写事务**（`BEGIN IMMEDIATE`）。
    ///
    /// 为什么不直接用 `pool.begin()`：那是 deferred 事务——若事务内**先读后写**，
    /// 而两次操作之间另一个连接提交了写，SQLite 返回 `SQLITE_BUSY_SNAPSHOT`（code 517）。
    /// 它**不是锁等待**，`busy_timeout` 对同一个快照重试多少次都不会成功，事务直接失败。
    /// 单任务串行时读-写之间没有竞争者，所以从未暴露；但同项目并发跑多个任务
    /// （决策 98 准入允许）是常态用法，主流程票 09 的浏览器用例③ 实测挂在这里：
    /// 并发执行器的多步事务里报「database is locked」→ 节点被重试耗尽。
    ///
    /// `BEGIN IMMEDIATE` 在建事务时就取写锁，把冲突变成**普通锁等待**（由
    /// `busy_timeout` 兜住），从根上消掉快照升级失败这一类。代价是事务并行度下降，
    /// 与本应用「单机单进程 + SQLite」的定位一致（决策 13 / 127）。
    pub(crate) async fn begin_write(&self) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
        Ok(self.pool().begin_with("BEGIN IMMEDIATE").await?)
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    /// 唯一时钟源：所有 `created_at` / `updated_at` 都经它写入（决策 64 / 143）。
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// 调整会话截断阈值（executor 按 Settings 注入；测试用小值验证截断）。
    pub fn set_conversation_max_chars(&mut self, max_chars: usize) {
        self.conversation_max_chars = max_chars;
    }

    /// 挂上离线通知出口（决策 268）：app 层在 `[notify].webhook_url` 在场时调一次；
    /// 不调 = 没配 = 整段关死。`&self` 而非 `&mut self`——锁在 `Arc` 字段里，
    /// 先创建的克隆同样看得见这次挂接。
    pub fn set_notifier(&self, notifier: Arc<crate::notify::WebhookNotifier>) {
        match self.notifier.write() {
            Ok(mut slot) => *slot = Some(notifier),
            Err(_) => tracing::error!("通知出口挂接失败（锁中毒），本轮没有离线通知"),
        }
    }

    /// 当前通知出口（`note_attention` 的触发点取用）。
    pub(crate) fn notifier(&self) -> Option<Arc<crate::notify::WebhookNotifier>> {
        self.notifier.read().ok().and_then(|slot| slot.clone())
    }

    /// 摘掉通知出口（决策 272⑧）：总开关关掉（或单元清掉后配置级也无通道）时的
    /// **活生效**路径。出口是启动时建一次的，开关要在运行期把整条通道关死，就得有
    /// 这条反向的路——今天之前没有它，「关掉」只能等重启。
    pub fn clear_notifier(&self) {
        match self.notifier.write() {
            Ok(mut slot) => *slot = None,
            Err(_) => tracing::error!("通知出口摘除失败（锁中毒），下一轮重启前仍可能出站"),
        }
    }
}

/// 时间戳 → DB TEXT（RFC3339）。
pub fn ts(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

/// DB TEXT → 时间戳（容忍 sqlx 的几种写法）。
///
/// 执行语义字段：全部格式失败 → **报错**，绝不伪造"现在"——伪造的时间戳会让
/// 超时 / 停滞判定失真，且问题被掩盖。
pub fn parse_ts(raw: &str) -> Result<DateTime<Utc>> {
    if let Ok(v) = DateTime::parse_from_rfc3339(raw) {
        return Ok(v.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(v) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
            return Ok(v.and_utc());
        }
    }
    if let Ok(v) = DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f%:z") {
        return Ok(v.with_timezone(&Utc));
    }
    Err(crate::Error::Validation(format!("无法解析时间戳：{raw}")))
}

/// 序列化 pending reason。
pub(crate) fn encode_pending(reason: &Option<PendingReason>) -> Option<String> {
    reason.as_ref().and_then(|r| serde_json::to_string(r).ok())
}

/// 反序列化 pending reason。JSON 损坏 → **报错**（它决定游标的执行语义与动作集）。
pub(crate) fn decode_pending(raw: Option<String>) -> Result<Option<PendingReason>> {
    raw.map(|s| Ok(serde_json::from_str::<PendingReason>(&s)?))
        .transpose()
}

/// 反序列化 stage（执行语义字段：DB 中非法值 → 报错而不是静默兜底）。
pub(crate) fn decode_stage(raw: &str) -> Result<Stage> {
    Stage::from_str(raw)
}

pub(crate) fn decode_node(raw: &str) -> Result<Node> {
    Node::from_str(raw)
}

/// 观测类枚举（trigger / command source 等不参与执行语义的字段）：
/// 非法值 → `warn` + 兜底，不让一条坏观测行打垮整个查询。
pub(crate) fn decode_lossy<T>(raw: &str, what: &str, fallback: T) -> T
where
    T: std::str::FromStr<Err = crate::Error>,
{
    match T::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{what} 存在非法值 {raw:?}（{e}），按兜底值处理");
            fallback
        }
    }
}
