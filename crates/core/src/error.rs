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
}

pub type Result<T> = std::result::Result<T, Error>;
