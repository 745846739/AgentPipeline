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

    /// 策略拒绝：文件工具路径（决策 104）与 `run_command` 出口（决策 179，票 12）。
    ///
    /// **不是系统级沙箱**：文件侧只约束 6 个文件工具，出口侧只约束 agent 经 `run_command`
    /// 主动发起的调用（子进程自行联网管不住）。两处都是「让直白动作可见、可拦」，
    /// 不是边界保证——残余风险见 `docs/operations.md` §12.15。
    #[error("策略拒绝：{0}")]
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
        /// 稳定标识（`llm_auth` / `llm_model_not_found` / `llm_network` / `llm_context_window` /
        /// `llm_quota`）。
        kind: String,
        /// 中文可操作提示（写入 `pending.message`）。
        message: String,
        /// 原始错误串（HTTP 状态 + 供应商返回体预览），供排查。
        raw: String,
    },

    /// 技能来源失败（决策 194 裁决⑦）：带**可归因的类别**，八类互不混淆。
    ///
    /// 网络抖动该重试、仓名写错该改名、那个 commit 不存在该换锚、仓里没有这个目录该换
    /// 目标、仓不可读该查权限、对象哈希不符该怀疑中间人、仓不在名单该改配置、超体积该
    /// 换个小仓——八种动作毫无交集，混成一个「市场错误」等于没报错。HTTP 状态码会撞
    /// （多个类别都是 404），故 `kind` 是**唯一**的分辨依据。类别常量见
    /// [`crate::agent::repo`] 的 `KIND_*`。
    #[error("{message}")]
    Market {
        /// 稳定标识，取值见 [`crate::agent::repo`] 的 `KIND_*`
        /// （`market_network` / `repo_not_found` / `commit_not_found` / `skill_not_found` /
        /// `repo_unreadable` / `digest_mismatch` / `repo_not_allowed` / `download_too_large`）。
        kind: String,
        /// 中文可操作提示（面向用户）。
        message: String,
        /// 原始诊断（git 失败串 / 期望与实际对象哈希 / 目录名 / 字节数），供排查。
        raw: String,
    },

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("未实现：{0}")]
    NotImplemented(String),

    /// 执行体被**主动中止**（决策 226）：调度器判节点超时后，通知执行体自己收口。
    ///
    /// 与其余错误的区别在**归属**，不在严重程度：它表示「这一轮已经不算数了」，而不是
    /// 「这个节点失败了」。故上层不按节点失败处置——不挂 `pending`、不再重试本节点
    /// （那会把判超时那边刚放出去的那次重试立刻打回去）；run 行的终态与重试记账归
    /// `scheduler::handle_timeout`，执行体只把自己手里那份**用量**补记上去。
    #[error("已中止：{0}")]
    Cancelled(String),
}

impl Error {
    /// 是否为「执行体按中止请求收口」（决策 226）。
    ///
    /// 调用方要按它分流：中止**不是**节点失败，别当失败处置。
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Cancelled(_))
    }

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
