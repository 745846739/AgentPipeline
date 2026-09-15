//! 统一错误类型（决策：thiserror 用于库，anyhow 用于二进制入口）。

/// 库层统一错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("配置错误：{0}")]
    Config(String),

    #[error("数据库错误：{0}")]
    Db(#[from] sqlx::Error),

    #[error("迁移错误：{0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("游标错误：{0}")]
    Cursor(String),

    #[error("任务错误：{0}")]
    Task(String),

    #[error("git 错误：{0}")]
    Git(String),

    /// FileToolPolicy 拒绝（决策 104）。不是系统级沙箱，只约束文件工具。
    #[error("文件工具策略拒绝：{0}")]
    PolicyDenied(String),

    /// 需要显式 cursor_id 但缺失/歧义（决策 91）→ API 层映射为 409。
    #[error("冲突：{0}")]
    Conflict(String),

    #[error("校验错误：{0}")]
    Validation(String),

    /// 生产 LLM 适配器调用失败（HTTP 错误 / 响应解析失败 / provider 配置缺失，票 13）。
    #[error("LLM 调用失败：{0}")]
    Llm(String),

    /// 可归因的 LLM 配置类失败（主流程票 03）：`message` 是中文可操作提示，
    /// `raw` 保留原始诊断（进 `pending.context.diagnostic`，不进 message）。
    ///
    /// 与 `Llm` 的区别：`Llm` 是「发生了什么」；`LlmClassified` 还回答了
    /// 「该去改什么」。未识别的错误一律仍走 `Llm`——宁可退回原始串，不误标类别。
    #[error("{message}")]
    LlmClassified {
        /// 稳定标识（`llm_auth` / `llm_model_not_found` / `llm_network` / `llm_context_window`）。
        kind: String,
        /// 中文可操作提示（写入 `pending.message`）。
        message: String,
        /// 原始错误串（HTTP 状态 + 供应商返回体预览），供排查。
        raw: String,
    },

    /// 技能市场失败（票 10）：带**可归因的类别**，四类互不混淆。
    ///
    /// 网络失败该重试、摘要不符该怀疑中间人、来源未放行该改配置、索引畸形该找 registry
    /// 维护者——四种动作毫无交集，混成一个「市场错误」等于没报错。类别常量见
    /// [`crate::agent::market`] 的 `KIND_*`。
    #[error("{message}")]
    Market {
        /// 稳定标识（`market_network` / `market_digest_mismatch` /
        /// `market_source_not_allowed` / `market_index_malformed` / `market_not_found`）。
        kind: String,
        /// 中文可操作提示（面向用户）。
        message: String,
        /// 原始诊断（HTTP 状态 / 期望与实际摘要 / 索引片段），供排查。
        raw: String,
    },

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("未实现：{0}")]
    NotImplemented(String),
}

impl Error {
    /// API 层是否需要把这个错误映射为 409。
    pub fn is_conflict(&self) -> bool {
        matches!(self, Error::Conflict(_))
    }

    /// 若为可归因的 LLM 配置类失败，返回 `(类别标识, 原始诊断)`（主流程票 03）。
    pub fn llm_classified(&self) -> Option<(&str, &str)> {
        match self {
            Error::LlmClassified { kind, raw, .. } => Some((kind, raw)),
            _ => None,
        }
    }

    /// 若为技能市场失败，返回 `(类别标识, 原始诊断)`（票 10）。
    ///
    /// 与 [`Error::llm_classified`] 并列：市场失败也带稳定类别，调用方（API 层分类映射、
    /// 测试断言）不该各自手写一遍 `match Error::Market { .. }`。
    pub fn market_kind(&self) -> Option<(&str, &str)> {
        match self {
            Error::Market { kind, raw, .. } => Some((kind, raw)),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
