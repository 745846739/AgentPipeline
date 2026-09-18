//! 全局配置与阶段级覆盖（docs/overview.md §3、docs/agents.md §10.6）。
//!
//! config.toml 只保留 `[server]` / `[pipeline]` / `[logging]` / `[prompts]`（决策 56）；
//! provider 与阶段配置存 DB（决策 22 / 111）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{Provider, StageConfig};
use crate::{Error, Result};

/// v1 代码支持的适配器集合（决策 103：硬编码常量，改它要发版）。
pub const SUPPORTED_ADAPTERS: [&str; 3] = ["openai", "anthropic", "deepseek"];

/// 全局参数（§3 全表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub validate_retry_max: u32,
    pub agent_retry_max: u32,
    pub tool_retry_max: u32,
    pub node_idle_timeout_sec: u64,
    pub node_max_duration_sec: u64,
    pub tool_timeout_sec: u64,
    pub test_command_timeout_sec: u64,
    pub adaptive_timeout_enabled: bool,
    pub pending_resume_cooldown_sec: u64,
    pub pending_reminder_hours: u64,
    pub pending_timeout_hours: u64,
    pub tick_interval_sec: u64,
    pub conversation_max_chars: usize,
    pub conversation_retention_days: u64,
    /// 唯一的工具结果阈值（决策 110）。
    pub offload_threshold_tokens: usize,
    pub context_soft_limit_ratio: f64,
    pub keep_recent_rounds: usize,
    pub context_hard_limit_ratio: f64,
    pub semantic_conflict_check: bool,
    pub cross_family_judge: bool,
    pub conflict_overlap_threshold: usize,
    pub max_concurrent_tasks: usize,
    pub allow_dirty_worktree_merge: bool,
    /// `run_command` 的出口放行主机（决策 179，票 12）：精确主机 / `*.example.com` / `*`。
    ///
    /// **默认空 = 只放行回环**。忘配的代价是某条命令被拒并在报错里说明怎么放行，配宽的代价
    /// 是静默放行陌生目标——两个方向不对称，故默认取保守侧（票 12 的硬要求）。
    pub egress_allow_hosts: Vec<String>,
    /// 显式放行全部出口。默认 `false`：未配置时**不得**静默变成「全部放行」。
    pub egress_allow_all: bool,
    /// 环境层权限档位的**全局默认**（决策 206）。
    ///
    /// 缺省 `auto` = **等于现状**：环境层工具（文件 / 命令 / 技能拉取 / 子代理）直接执行，
    /// 与档位出现之前逐字相同。阶段级覆盖在 `stage_configs.env_mode`，值班长的缺省是
    /// `ask`（它的输入是人可以随便打的任意文本）——两层解析的唯一实现在
    /// [`crate::types::effective_env_mode`]。
    ///
    /// 写在这里而不是 UI 里：改它等于改全机所有阶段的行为，那是一次深思熟虑的编辑，
    /// 不是一次点击（照 `allow_dirty_worktree_merge` 那一批的做法）。
    pub env_mode: crate::types::EnvMode,
    /// **事件的新鲜窗口**（分钟，决策 209②/③，票 05）：多久之内发生的事才值得写进
    /// 值班长待办。它同时是「同一任务在窗口内再次 pending」的计数窗口——两件事本来就是
    /// 同一个数（「多久之内算同一件事」）。
    ///
    /// 为什么要窗口：待办表的去重键含事件发生时刻，而「写待办」是每一 tick 都跑的。
    /// 没有窗口，一个三天前就 pending 的任务会被反复写、反复唤醒。
    pub watch_event_window_minutes: u64,
    /// **卡住的宽限**（分钟，票 05）：两条「调度器与台账之间的缝」用它判——
    /// `scheduler_no_effect`（run 已终态而游标仍 active）与 `owner_stuck`
    /// （任务 running 但长时间没有 run 心跳）。默认 10 分钟：低于它时还在正常重试的
    /// 时间范围内，报出来只会是噪声。
    pub watch_owner_stuck_minutes: u64,
    /// **值守轮的去抖窗口**（秒，决策 209④ / 票 06）：窗口内攒批，到期才唤醒一次。
    ///
    /// 它换来的是「不为一件事吵两次」；代价是响得慢一点——夜里值守最不缺的就是时间。
    pub watch_debounce_sec: u64,
    /// **同任务冷却**（分钟，决策 209⑤ / 票 07）：刚被处理过的任务，新事件不单独唤醒，
    /// 留在待办表里等冷却到期后合并播报。判据是「这个任务最近有没有被消费过的待办」。
    pub watch_task_cooldown_minutes: u64,
    /// **全局唤醒上限**（次/小时，决策 209⑤ / 票 07）。触顶时不再唤醒，但**不静默丢弃**：
    /// 留一行「本小时已达上限，N 条待办未播报」，待办不消费，下一小时继续。
    pub watch_max_wakes_per_hour: u64,
    /// **项目级 run 的空闲超时**（秒，决策 212 / 票 13）。
    ///
    /// 与节点超时**语义分开**（不是一个数）：项目级伪阶段没有任务、没有游标，它的生命周期
    /// 归 `POST /projects/analyze` 收尾——而那条路在**进程被杀 / 重启**时跑不到收尾，
    /// 于是留下一条跨重启永生的 `running` 行（2026-09-17 实证：三条 run 的活动时间冻结、
    /// 仍是 running）。这个超时就是那种行的终止者。
    ///
    /// 默认 900 秒：一次项目分析是「探测（纯代码）+ 一次 LLM 摘要」，15 分钟远够。
    pub project_run_idle_timeout_sec: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            validate_retry_max: 3,
            agent_retry_max: 3,
            tool_retry_max: 3,
            node_idle_timeout_sec: 300,
            node_max_duration_sec: 1800,
            tool_timeout_sec: 60,
            test_command_timeout_sec: 600,
            adaptive_timeout_enabled: false,
            pending_resume_cooldown_sec: 5,
            pending_reminder_hours: 24,
            pending_timeout_hours: 72,
            tick_interval_sec: 10,
            conversation_max_chars: 200_000,
            conversation_retention_days: 30,
            offload_threshold_tokens: 4000,
            context_soft_limit_ratio: 0.6,
            keep_recent_rounds: 5,
            context_hard_limit_ratio: 0.9,
            semantic_conflict_check: true,
            cross_family_judge: false,
            conflict_overlap_threshold: 0,
            max_concurrent_tasks: 5,
            allow_dirty_worktree_merge: false,
            egress_allow_hosts: Vec::new(),
            egress_allow_all: false,
            env_mode: crate::types::EnvMode::Auto,
            watch_event_window_minutes: 30,
            watch_owner_stuck_minutes: 10,
            watch_debounce_sec: 60,
            watch_task_cooldown_minutes: 30,
            watch_max_wakes_per_hour: 12,
            project_run_idle_timeout_sec: 900,
        }
    }
}

/// config.toml 的可选覆盖层（未写的项取 [`Settings::default`]）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PipelineOverrides {
    pub validate_retry_max: Option<u32>,
    pub agent_retry_max: Option<u32>,
    pub tool_retry_max: Option<u32>,
    pub node_idle_timeout_sec: Option<u64>,
    pub node_max_duration_sec: Option<u64>,
    pub tool_timeout_sec: Option<u64>,
    pub test_command_timeout_sec: Option<u64>,
    pub adaptive_timeout_enabled: Option<bool>,
    pub pending_resume_cooldown_sec: Option<u64>,
    pub pending_reminder_hours: Option<u64>,
    pub pending_timeout_hours: Option<u64>,
    pub tick_interval_sec: Option<u64>,
    pub conversation_max_chars: Option<usize>,
    pub conversation_retention_days: Option<u64>,
    pub offload_threshold_tokens: Option<usize>,
    pub context_soft_limit_ratio: Option<f64>,
    pub keep_recent_rounds: Option<usize>,
    pub context_hard_limit_ratio: Option<f64>,
    pub semantic_conflict_check: Option<bool>,
    pub cross_family_judge: Option<bool>,
    pub conflict_overlap_threshold: Option<usize>,
    pub max_concurrent_tasks: Option<usize>,
    pub allow_dirty_worktree_merge: Option<bool>,
    pub egress_allow_hosts: Option<Vec<String>>,
    pub egress_allow_all: Option<bool>,
    /// 环境层档位（决策 206）。**用字符串接**：`deny_unknown_fields` +
    /// 枚举反序列化会把 `env_mode = "Auto"` 报成一句难读的 serde 错误，而这里要的是一句
    /// 「只能是 auto / ask / deny」——解析与校验在 [`PipelineOverrides::apply`] 里做。
    pub env_mode: Option<String>,
    pub watch_event_window_minutes: Option<u64>,
    pub watch_owner_stuck_minutes: Option<u64>,
    pub watch_debounce_sec: Option<u64>,
    pub watch_task_cooldown_minutes: Option<u64>,
    pub watch_max_wakes_per_hour: Option<u64>,
    pub project_run_idle_timeout_sec: Option<u64>,
}

impl PipelineOverrides {
    /// 合并到全局默认，得到有效设置。
    pub fn apply(self, base: &Settings) -> Settings {
        let mut s = base.clone();
        macro_rules! set {
            ($($f:ident),* $(,)?) => {
                $( if let Some(v) = self.$f { s.$f = v; } )*
            };
        }
        set!(
            validate_retry_max,
            agent_retry_max,
            tool_retry_max,
            node_idle_timeout_sec,
            node_max_duration_sec,
            tool_timeout_sec,
            test_command_timeout_sec,
            adaptive_timeout_enabled,
            pending_resume_cooldown_sec,
            pending_reminder_hours,
            pending_timeout_hours,
            tick_interval_sec,
            conversation_max_chars,
            conversation_retention_days,
            offload_threshold_tokens,
            context_soft_limit_ratio,
            keep_recent_rounds,
            context_hard_limit_ratio,
            semantic_conflict_check,
            cross_family_judge,
            conflict_overlap_threshold,
            max_concurrent_tasks,
            allow_dirty_worktree_merge,
            egress_allow_hosts,
            egress_allow_all,
            watch_event_window_minutes,
            watch_owner_stuck_minutes,
            watch_debounce_sec,
            watch_task_cooldown_minutes,
            watch_max_wakes_per_hour,
            project_run_idle_timeout_sec,
        );
        // 非法值由 [`Config::validate`] 在解析期拦下（fail fast），故这里只做「认得出就采用」
        // ——两处都报错会让同一个错误有两个出口，而这里没有 `Result` 可返回。
        if let Some(mode) = self
            .env_mode
            .as_deref()
            .and_then(crate::types::EnvMode::parse)
        {
            s.env_mode = mode;
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub port: u16,
    /// 绑定地址（§10.6.5：`[server] host`）。只允许 IP 字面量。
    pub host: String,
    /// 额外放行的跨源写 origin 白名单（决策 157）。缺省恒含
    /// `http://127.0.0.1:{port}` / `http://localhost:{port}`（决策 128），本键
    /// 用于局域网等**显式扩权**；值须为 `scheme://host[:port]`，尾部斜杠在
    /// 归一时剥除，非法值 fail fast（决策 134）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            port: 8788,
            host: "127.0.0.1".to_string(),
            allowed_origins: Vec::new(),
        }
    }
}

/// 归一并校验一个跨源白名单 origin（决策 157）：`scheme://host[:port]`，小写化、
/// 剥尾部斜杠。`[server] allowed_origins`（解析期）与 CLI `--allowed-origin`
/// （启动期）共用，非法值一律报错。
pub fn normalize_origin(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| format!("origin 缺少 scheme（需 http:// 或 https://）：{raw:?}"))?;
    let scheme = scheme.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("origin scheme 仅支持 http/https：{raw:?}"));
    }
    if rest.is_empty() || rest.contains(['/', '\\', ' ', '?', '#']) {
        return Err(format!(
            "origin 只能是 scheme://host[:port]，不含路径：{raw:?}"
        ));
    }
    Ok(format!("{scheme}://{rest}").to_lowercase())
}

/// `[logging] format` 的取值（§10.6.5，票 16）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// 多行、带缩进的人读格式（缺省）。
    #[default]
    Pretty,
    /// 单行紧凑格式。
    Compact,
    /// 每行一条 JSON（供机器采集）。
    Json,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Pretty => "pretty",
            LogFormat::Compact => "compact",
            LogFormat::Json => "json",
        }
    }
}

/// `[logging]`（§10.6.5，票 16 修正键名）。
///
/// 键名与文档一致：`level` / `format` / `file`。旧代码结构体的 `json_file`
/// 作为**已废弃键**保留兼容（见 [`LoggingConfig::json_file`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    /// 日志格式；未设置时由 `json_file` 推导，最终缺省 `pretty`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<LogFormat>,
    /// 日志文件路径（`~` 可展开，相对路径按 home 根解析）；空 = 不落文件。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// **已废弃**（决策 56 时代的旧键）：`json_file = true` 等价 `format = "json"`。
    /// 与 `format` 同时出现属冲突配置 → 解析期 fail fast（`Config::from_toml`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_file: Option<bool>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".to_string(),
            format: None,
            file: None,
            json_file: None,
        }
    }
}

impl LoggingConfig {
    /// 有效格式：显式 `format` 优先；否则由旧键 `json_file` 推导；最终 `pretty`。
    pub fn effective_format(&self) -> LogFormat {
        match (self.format, self.json_file) {
            (Some(f), _) => f,
            (None, Some(true)) => LogFormat::Json,
            (None, Some(false)) => LogFormat::Pretty,
            (None, None) => LogFormat::Pretty,
        }
    }

    /// 解析后的日志文件路径；未配置或空白 → `None`（只写标准输出）。
    pub fn resolved_file(&self, home_root: &Path) -> Option<PathBuf> {
        let raw = self.file.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(resolve_config_path(raw, home_root))
    }
}

/// 展开配置里的路径（§10.6.4 同口径）：
/// - `~` / `~/x` → 用户家目录；
/// - 绝对路径原样；
/// - 相对路径按 `home_root` 解析。
pub fn resolve_config_path(raw: &str, home_root: &Path) -> PathBuf {
    let raw = raw.trim();
    if raw == "~" {
        return user_home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return user_home_dir().join(rest);
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        home_root.join(p)
    }
}

/// 用户家目录（`$HOME`；缺失时回退当前目录，与 [`crate::home::agentpipeline_home`] 同口径）。
fn user_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 发现"可用 skill"（决策 47，经决策 170 / 172 扩展、**决策 185 收窄为单一来源**）。
///
/// 今天只有**用户 markdown**（技能根下的 `{name}/SKILL.md`；内嵌技能随决策 172① 退场、
/// PATH 工具型技能随决策 185 退场）。技能正文的注入见 [`crate::agent::skills::resolve`]
/// 与 [`crate::agent::prompts::build_system_prompt`]。
///
/// `skills_root` 是技能根**本身**（默认 `{home}/skills`，可由 `[skills] dir` 覆盖，
/// 决策 172）——本函数与 [`crate::agent::skills::discover`] 走同一入口，不新增发现路径。
pub fn discover_available_skills(skills_root: &Path) -> Vec<String> {
    crate::agent::skills::skill_names(skills_root)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PromptsConfig {
    /// 覆盖 prompt 的目录；缺省时用 `{home}/prompts`（票 16：已接入 `Home::prompts_dir`）。
    pub dir: Option<String>,
}

impl PromptsConfig {
    /// 解析后的覆盖目录；未配置或空白 → `None`（回落 `{home}/prompts`）。
    pub fn resolved_dir(&self, home_root: &Path) -> Option<PathBuf> {
        let raw = self.dir.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(resolve_config_path(raw, home_root))
    }
}

/// `[skills]`：技能根覆盖（决策 172，修订决策 47）。
///
/// 照 [`PromptsConfig`] 的先例：相对路径按 home 根解析、`~` 展开、空白 → `None`。
/// 未配置时技能根仍是 `{home}/skills`，行为逐字不变。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
    /// 覆盖技能根的目录；缺省时用 `{home}/skills`。
    ///
    /// 典型用法是把它指到已有的技能生态目录，如 `~/.zcode/skills`。
    pub dir: Option<String>,
}

impl SkillsConfig {
    /// 解析后的技能根；未配置或空白 → `None`（回落 `{home}/skills`）。
    pub fn resolved_dir(&self, home_root: &Path) -> Option<PathBuf> {
        let raw = self.dir.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(resolve_config_path(raw, home_root))
    }
}

/// `[market]`：技能市场的来源**仓名单**（决策 194；此前是 registry 的 origin 白名单，
/// 见决策 172⑤ / 187）。
///
/// 姿态不变，判定对象变了：**默认空，即默认不从任何仓安装**。这是保守方向上的默认——
/// 忘配的代价是装不上（用户立刻发现并去配置），配宽的代价是静默从陌生仓装上引导 agent 的正文。
///
/// ## 判定按 `owner/repo`，不再按 origin
///
/// GitHub 模式下字节的来源恒为 `github.com`，按 origin 放行等于放行**全世界任何作者的任何仓**。
/// 故信任单元是仓名本身，合法性判定收在 [`crate::agent::repo::RepoId::parse`] 一处
/// （配置解析、界面保存、读取层共用它）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MarketConfig {
    /// 放行的技能来源仓（`owner/repo`，如 `obra/superpowers`）。
    ///
    /// 一个仓里可能有很多技能（实测 `wshobson/agents` 有 183 个），放行的粒度就是仓。
    pub github_repos: Vec<String>,

    /// **已退场的旧键**（决策 194 之前那份 registry 的 origin 白名单）。
    ///
    /// 留着这个字段只有一个目的：让它报出**一句能照着改的话**。直接删掉字段的话，
    /// `deny_unknown_fields` 会让启动失败于「unknown field `allowed_sources`」——那虽然也是
    /// fail fast（正确的姿态），但用户不知道拿什么替代；而这一页上"改了没生效"的代价特别高
    /// （它是唯一的安全控制，配错了却以为配上了最坏）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_sources: Option<Vec<String>>,
}

impl MarketConfig {
    /// 归一后的放行仓集合（顺序保留、按仓名去重）。
    ///
    /// 用户把同一个仓写两遍是常见手误（大小写还常常不一致），去重后不表现为「配了两条却只有一条生效」
    /// 这种隐式行为差异。比较按大小写不敏感——GitHub 认的是同一个仓。
    pub fn resolved_repos(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for raw in &self.github_repos {
            if let Ok(id) = crate::agent::repo::RepoId::parse(raw) {
                let slug = id.slug();
                if !out.iter().any(|seen| seen.eq_ignore_ascii_case(&slug)) {
                    out.push(slug);
                }
            }
        }
        out
    }
}

/// 校验并归一**一组**仓名（配置与界面共用同一条口径，决策 187 的两级结构由决策 194 继承）。
///
/// 界面上的仓名单编辑器与 `config.toml` 的 `[market] github_repos` 会写进同一个语义位，
/// 两处各写一份校验必然漂移——而那正是安全相关的一处（放行一个仓 = 允许从它下载引导 agent
/// 的正文）。故合法性判定收在 [`crate::agent::repo::RepoId::parse`]，这里只做"逐条归一 +
/// 去重 + 报出是哪一项为什么"。
///
/// 非法项**fail fast，不静默丢弃**：丢一条的表现是"我明明配了它却说没放行"。
pub fn validate_market_repos(raw: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let id = crate::agent::repo::RepoId::parse(item)
            .map_err(|e| Error::Config(format!("技能来源仓不合法（{item}）：{e}")))?;
        let slug = id.slug();
        if !out.iter().any(|seen| seen.eq_ignore_ascii_case(&slug)) {
            out.push(slug);
        }
    }
    Ok(out)
}

/// 完整配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub pipeline: PipelineOverrides,
    pub logging: LoggingConfig,
    pub prompts: PromptsConfig,
    pub skills: SkillsConfig,
    pub market: MarketConfig,
}

impl Config {
    /// 从 TOML 文本解析。
    ///
    /// 未知键由 `deny_unknown_fields` 直接拒绝（沿用既有 fail fast 姿态，决策 47 /
    /// 103 / 134）；解析后做**跨字段冲突校验**（如 `format` 与已废弃的 `json_file`
    /// 同时出现），不静默取其一。
    pub fn from_toml(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text)
            .map_err(|e| Error::Config(format!("config.toml 解析失败：{e}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// 跨字段一致性校验（决策 47 / 103 / 134 的 fail fast 姿态）。
    pub fn validate(&self) -> Result<()> {
        if self.logging.format.is_some() && self.logging.json_file.is_some() {
            return Err(Error::Config(
                "[logging] 的 `format` 与已废弃的 `json_file` 不能同时配置；\
                 请只保留 `format`（json_file 仅为旧配置兼容）"
                    .into(),
            ));
        }
        for origin in &self.server.allowed_origins {
            normalize_origin(origin)
                .map_err(|e| Error::Config(format!("[server] allowed_origins 校验失败：{e}")))?;
        }
        // 已退场的旧键：`[market] allowed_sources`（决策 194 之前那份 registry 的 origin 白名单）。
        // **拦在这里而不是靠 `deny_unknown_fields`**：那条会报「unknown field `allowed_sources`」，
        // 虽然也是 fail fast（正确姿态），但用户不知道拿什么替代。报文要把新旧口径说清，
        // 因为这一页是唯一的安全控制——配错了却以为配上了是最坏的情形。
        if let Some(old) = self.market.allowed_sources.as_ref() {
            return Err(Error::Config(format!(
                "[market] allowed_sources 已被 [market] github_repos 取代（决策 194）：\
                 技能来源不再是自定的 registry 索引，而是一个 GitHub 仓。\
                 把那几个来源换算成 owner/repo 写进 github_repos\
                 （例如 github_repos = [\"obra/superpowers\"]）。旧值仍在配置里：{old:?}"
            )));
        }
        // 仓名单同样在解析期 fail fast：写错的仓名若放过去，表现为「安装时来源未放行」
        // 这种运行期错误，用户得回头猜配置哪里错了（八类失败要分得开）
        validate_market_repos(&self.market.github_repos)
            .map_err(|e| Error::Config(format!("[market] github_repos 校验失败：{e}")))?;
        // 出口放行清单同样在解析期 fail fast（票 12）：写错的条目若放过去，表现为运行期
        // 「明明列了还是被拒」，用户得回头猜（与上一条同理）
        if let Some(hosts) = self.pipeline.egress_allow_hosts.as_ref() {
            for host in hosts {
                crate::agent::egress::check_allow_host(host).map_err(|e| {
                    Error::Config(format!("[pipeline] egress_allow_hosts 校验失败：{e}"))
                })?;
            }
        }
        // 环境层档位（决策 206）同样在解析期 fail fast：写错一个档位名而它悄悄退回
        // `auto`，等于把一次收紧的意图变成一次放松——这个方向不能靠猜。
        //
        // 这一层是**全局默认**，而真实阶段的档位取自它，故 `ask` 在这里就是「所有阶段都
        // 变成 ask」——那不是收紧而是静默收掉所有环境写动作（流水线无人按提议）。
        // 故这一层只收 `auto` / `deny`；`ask` 是值班长行的档位（规格 §4）。
        if let Some(raw) = self.pipeline.env_mode.as_ref() {
            let mode = crate::types::EnvMode::parse_or_message(raw)
                .map_err(|e| Error::Config(format!("[pipeline] {e}")))?;
            if mode == crate::types::EnvMode::Ask {
                return Err(Error::Config(
                    "[pipeline] env_mode 不能是 ask：这一层是所有阶段的全局默认，而流水线节点\
                     无人值守（ask 在那里没有提议通道，等于静默收掉全部环境写动作）——\
                     要收紧请写 deny；ask 请配在 stage_configs 的 foreman 行上"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// 从文件加载；文件不存在时用默认配置（首次启动）。
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        Config::from_toml(&text)
    }

    /// 有效全局参数。
    pub fn settings(&self) -> Settings {
        self.pipeline.clone().apply(&Settings::default())
    }
}

// ─────────────────────── 超时有效值（决策 66 / 75）───────────────────────

/// 节点级覆盖（决策 66 第三层）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeTimeouts {
    pub idle_timeout_sec: Option<u64>,
    pub max_duration_sec: Option<u64>,
}

/// 有效空闲超时 = 节点级 > 阶段级 > 全局（决策 66）。
pub fn effective_idle_timeout(global: u64, stage_override: Option<u64>, node: NodeTimeouts) -> u64 {
    node.idle_timeout_sec.or(stage_override).unwrap_or(global)
}

/// 有效绝对超时 = 节点级 > 阶段级 > 全局（决策 66）。
pub fn effective_max_duration(global: u64, stage_override: Option<u64>, node: NodeTimeouts) -> u64 {
    node.max_duration_sec.or(stage_override).unwrap_or(global)
}

/// 从阶段配置读节点级覆盖（`node_overrides_json`）。
pub fn node_timeouts(stage_cfg: Option<&StageConfig>, node: &str) -> NodeTimeouts {
    let Some(cfg) = stage_cfg else {
        return NodeTimeouts::default();
    };
    let Some(value) = cfg.node_overrides_json.as_ref() else {
        return NodeTimeouts::default();
    };
    let Some(node_obj) = value.get(node) else {
        return NodeTimeouts::default();
    };
    NodeTimeouts {
        idle_timeout_sec: node_obj.get("idle_timeout_sec").and_then(|v| v.as_u64()),
        max_duration_sec: node_obj.get("max_duration_sec").and_then(|v| v.as_u64()),
    }
}

/// 从阶段配置读**节点级**技能声明（`node_overrides_json[node].skills`，决策 170）。
///
/// 语义与阶段级 `skills_json` 一致，有效集 = `mandatory ∪ 阶段级 ∪ 节点级`（只增不减，
/// §10.6.4 并集规则）。这一层解决的是「同一阶段的不同节点需要不同知识」——
/// 例如 architect-design 的 validate_input 要拷问、execute 要综合成规格，而
/// `skills_json` 是阶段级的、无法区分节点。
///
/// 字段形态为 `string | {name, mode, trusted}` 混合数组（决策 172④，票 05）；
/// 非法形态（非字符串元素、未知 `mode`、未信任 + full）→ [`Error::Config`]，
/// 且报文定位到阶段与节点。
pub fn node_skills(
    stage_cfg: Option<&StageConfig>,
    node: &str,
) -> Result<Vec<crate::agent::skills::SkillDecl>> {
    let Some(value) = stage_cfg
        .and_then(|c| c.node_overrides_json.as_ref())
        .and_then(|v| v.get(node))
        .and_then(|n| n.get("skills"))
    else {
        return Ok(Vec::new());
    };
    let stage = stage_cfg.map(|c| c.stage.as_str()).unwrap_or("(未知阶段)");
    parse_skill_decls(value, &format!("阶段 {stage} 节点 {node}"))
}

/// 阶段级技能声明（`skills_json`，决策 172④，票 05）。
pub fn stage_skills(
    stage_cfg: Option<&StageConfig>,
) -> Result<Vec<crate::agent::skills::SkillDecl>> {
    let Some(value) = stage_cfg.and_then(|c| c.skills_json.as_ref()) else {
        return Ok(Vec::new());
    };
    let stage = stage_cfg.map(|c| c.stage.as_str()).unwrap_or("(未知阶段)");
    parse_skill_decls(value, &format!("阶段 {stage}"))
}

/// 解析技能声明数组（决策 172④，票 05）：字段形态由 `string[]` 扩展为
/// `string | {name, mode, trusted}` 的混合数组。
///
/// - 裸字符串 → `{mode: "full", trusted: false}`（**向后兼容今天的配置行**，零迁移）；
/// - 对象形态：`mode` 缺省为 `full`，`trusted` 缺省为 `false`；
/// - **既不是字符串也不是对象的元素按「不是声明」忽略**（沿用票 01 的宽松口径，
///   `42` / `null` 这类杂值不因本票改变行为）；但对象一旦成形，字段就必须合法——
///   缺 `name`、`mode` 非法、未信任 + full 一律 [`Error::Config`]，`where_` 把报错
///   定位到阶段 / 节点。
///
/// **未信任技能不得以 `full` 保存**（选型 D）：写入时拒绝并报错。这道门只对**显式对象**
/// 生效——裸字符串是信任概念出现之前手写的配置行，若一并拒绝则今天所有配置行都会失效，
/// 与票面的「零迁移」冲突；它们按 `full` 解释且不加信任位。
pub fn parse_skill_decls(
    value: &serde_json::Value,
    where_: &str,
) -> Result<Vec<crate::agent::skills::SkillDecl>> {
    use crate::agent::skills::{SkillDecl, SkillMode};

    let Some(items) = value.as_array() else {
        // 非数组（含旧版单字符串形态）按空处理——工具型字段的既有宽松口径（票 01）
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in items {
        // 裸字符串：向后兼容路径
        if let Some(name) = item.as_str() {
            out.push(SkillDecl::from_bare(name));
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue; // 杂值不是声明，忽略（票 01 口径不变）
        };
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Config(format!("{where_} 的技能声明缺少 name：{item}")))?
            .to_string();
        let mode = match obj.get("mode").and_then(|v| v.as_str()) {
            None => SkillMode::Full,
            Some(raw) => SkillMode::parse(raw).ok_or_else(|| {
                Error::Config(format!(
                    "{where_} 的技能 {name} 的 mode 非法：{raw}（须为 full 或 name）"
                ))
            })?,
        };
        let trusted = obj
            .get("trusted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if mode == SkillMode::Full && !trusted {
            return Err(Error::Config(format!(
                "{where_} 的技能 {name} 未受信任，不得以 full 模式注入全文；\
                 请先确认信任，或改用 mode = \"name\"（决策 172④）"
            )));
        }
        out.push(SkillDecl {
            name,
            mode,
            trusted,
        });
    }
    Ok(out)
}

// ───────────────────── 信任转换（票 11 的显式动作）─────────────────────

/// 把一份阶段配置里引用 `name` 的声明**全部**改写为「已信任」或「未信任」。
///
/// 返回改写过的那份配置；`None` 表示这份配置没引用该技能（**不动它**）。
///
/// ## 为什么信任态就地改写而不是另存一份
///
/// 信任是 `SkillDecl` 的字段（票 05），且**写入侧有门**：未信任 + `full` 被
/// [`parse_skill_decls`] 拒绝。若另建一份「信任表」，同一个问题就有了两个答案，而这是
/// **安全相关**的判定（未信任不得全文注入）——两个答案意味着必然有一条路径判错。故本函数
/// 只做一件事：把已有的声明换成新的信任态，其余字段（`mode` / 其他技能 / 其他配置项）一律不动。
///
/// ## 降信任时 `full` 必须变 `name`，且**不静默降级**
///
/// `trusted = false` 撞上一条 `mode: "full"` 声明时，落盘会得到一份**启动时就 fail fast**
/// 的配置（票 05 的门）。两条路：静默把 `full` 改成 `name`，或拒绝并让用户自己改。
/// 取**拒绝**——静默改注入模式会悄悄停掉一个正在生效的知识源，与「卸载被引用的技能不得
/// 静默降级」（票 09）是同一条纪律：**形态变更必须是用户看见的动作**。
///
/// 裸字符串声明（`["grill"]`，按 `{full, trusted:false}` 解释）在此**物化**为显式对象：
/// 它本来就按 `full` 解释，只有变成 `{name, mode:"full", trusted:true}` 才能表达「这条已信任」
/// ——裸字符串没有地方放 `trusted: true`。这是**语义等价**的改写（`full` 不变），不是降级。
pub fn set_skill_trust(
    cfg: &StageConfig,
    name: &str,
    trusted: bool,
) -> Result<Option<StageConfig>> {
    let mut out = cfg.clone();
    let mut touched = false;

    // 阶段级 `skills_json` 与每个节点的 `node_overrides_json[node].skills` 走**同一套**改写：
    // 票 05 的解析器就是这么共用的，两处判定不该分家
    let stage_where = format!("阶段 {}", cfg.stage);
    let (next, changed) = rewrite_decls(cfg.skills_json.as_ref(), name, trusted, &stage_where)?;
    if changed {
        out.skills_json = next;
        touched = true;
    }

    if let Some(overrides) = cfg.node_overrides_json.as_ref().and_then(|v| v.as_object()) {
        let mut new_overrides = overrides.clone();
        let mut nodes_touched = false;
        for (node, node_value) in overrides {
            let Some(node_obj) = node_value.as_object() else {
                continue;
            };
            let where_ = format!("阶段 {} 节点 {node}", cfg.stage);
            let (next, changed) = rewrite_decls(node_obj.get("skills"), name, trusted, &where_)?;
            if !changed {
                continue;
            }
            let mut updated = node_obj.clone();
            match next {
                Some(v) => {
                    updated.insert("skills".to_string(), v);
                }
                None => {
                    updated.remove("skills");
                }
            }
            new_overrides.insert(node.clone(), serde_json::Value::Object(updated));
            nodes_touched = true;
        }
        if nodes_touched {
            out.node_overrides_json = Some(serde_json::Value::Object(new_overrides));
            touched = true;
        }
    }

    Ok(touched.then_some(out))
}

/// 改写一份技能声明数组；`None` 表示原本没有该字段且无需新建。
///
/// 逐元素判断，**只碰引用 `name` 的那些**：其余元素（含暂时解析不通的杂值）原样保留，
/// 避免一次信任转换把用户手写的其他内容顺手规范化掉。
fn rewrite_decls(
    value: Option<&serde_json::Value>,
    name: &str,
    trusted: bool,
    where_: &str,
) -> Result<(Option<serde_json::Value>, bool)> {
    use crate::agent::skills::SkillMode;

    let Some(array) = value.and_then(|v| v.as_array()) else {
        return Ok((None, false));
    };
    let mut changed = false;
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(array.len());
    for item in array {
        // 裸字符串：只有名字相同时才物化（理由见 [`set_skill_trust`] 的文档）
        let is_bare_hit = item.as_str() == Some(name);
        let is_obj_hit = item
            .as_object()
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
            == Some(name);
        if !is_bare_hit && !is_obj_hit {
            out.push(item.clone());
            continue;
        }
        if is_bare_hit {
            // 裸字符串按 `{full, false}` 解释；改成未信任是无变化的，不改（少写一次盘）
            if !trusted {
                out.push(item.clone());
                continue;
            }
            changed = true;
            out.push(serde_json::json!({
                "name": name,
                "mode": "full",
                "trusted": true,
            }));
            continue;
        }
        let obj = item.as_object().expect("上面已判定是对象");
        let mode = obj.get("mode").and_then(|v| v.as_str()).unwrap_or("full");
        // 降信任撞上全文注入：拒绝，让用户自己决定改成名字态（不静默降级）
        if !trusted && mode == SkillMode::Full.as_str() {
            return Err(Error::Config(format!(
                "{where_} 的技能 {name} 正以 full 模式注入，改为未信任会让这份配置失效\
                 （未信任不得全文注入，决策 172④）。请先把该处的 mode 改为 \"name\"\
                 （正文改由 Skill 工具按需拉取），再撤销信任"
            )));
        }
        let already = obj
            .get("trusted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if already == trusted {
            out.push(item.clone());
            continue;
        }
        let mut updated = obj.clone();
        updated.insert("trusted".to_string(), serde_json::Value::Bool(trusted));
        changed = true;
        out.push(serde_json::Value::Object(updated));
    }
    Ok((Some(serde_json::Value::Array(out)), changed))
}
/// `run_command` 的超时上限（决策 75）：显式传值时取该值；未传时 test / merge 阶段取
/// `test_command_timeout_sec`，其余阶段取 `tool_timeout_sec`。
pub fn effective_run_command_timeout(
    settings: &Settings,
    stage: crate::types::Stage,
    explicit: Option<u64>,
) -> u64 {
    if let Some(t) = explicit {
        return t;
    }
    match stage {
        crate::types::Stage::Test | crate::types::Stage::Merge => settings.test_command_timeout_sec,
        _ => settings.tool_timeout_sec,
    }
}

// ─────────────────────── 启动校验（fail fast，决策 47 / 103 / 134）───────────────────────

/// 阶段声明的全部技能：阶段级 `skills_json` + 节点级 `node_overrides_json[node].skills`
/// （决策 170 / 172④）。
///
/// 返回 `(定位说明, 技能声明)`，定位说明用于报错——节点级要指明是哪个节点。
/// 非法声明形态（未知 `mode`、未信任 + full）在此直接 fail fast。
fn declared_skills(cfg: &StageConfig) -> Result<Vec<(String, crate::agent::skills::SkillDecl)>> {
    // 逐来源解析并**传播第一个错误**：写入路径要 fail fast，且报错顺序稳定
    let mut out = Vec::new();
    for (where_, decls) in declared_skill_sources(cfg) {
        for decl in decls? {
            out.push((where_.clone(), decl));
        }
    }
    Ok(out)
}

/// 逐来源（阶段级一处、每个节点级各一处）解析技能声明，**每个来源各自一个 `Result`**。
///
/// 拆到这一层是为了让 [`declared_skill_decls`] 能只丢掉**坏的那一处**：早先它调用
/// [`declared_skills`] 并在出错时 `unwrap_or_default()`，那会把整个阶段配置一并丢掉——
/// 于是一条坏的节点声明会让 `GET /skills` 的 `declared_in` 对**同阶段其他节点**的技能
/// 也报成「无人引用」，恰好在「卸载前看后果」这个用途上给出错误的安全感。
fn declared_skill_sources(
    cfg: &StageConfig,
) -> Vec<(String, Result<Vec<crate::agent::skills::SkillDecl>>)> {
    let stage = cfg.stage.as_str();
    let mut out: Vec<(String, Result<Vec<crate::agent::skills::SkillDecl>>)> = vec![(
        format!("阶段 {stage}"),
        parse_skill_decls_opt(cfg.skills_json.as_ref(), &format!("阶段 {stage}")),
    )];
    if let Some(overrides) = cfg.node_overrides_json.as_ref().and_then(|v| v.as_object()) {
        // 节点按名排序，报错顺序稳定（BTreeMap 的迭代序已有序，这里是显式保证）
        let mut nodes: Vec<&String> = overrides.keys().collect();
        nodes.sort();
        for node in nodes {
            let where_ = format!("阶段 {stage} 节点 {node}");
            let parsed = parse_skill_decls_opt(overrides[node].get("skills"), &where_);
            out.push((where_, parsed));
        }
    }
    out
}

/// 同 [`declared_skills`]，但**只丢掉解析失败的那一处来源**，不报错。
///
/// 供只读的界面路径使用（`GET /skills` 要回答「哪些阶段/节点引用了这个技能」，好让用户在
/// 卸载前看到后果）。这类查询**不该因为一条坏配置整体失败**——坏配置该由启动校验与
/// `PUT /stage-configs` 报错（写入路径 fail fast 才是对的）；若这里一并报错，用户反而失去
/// 了查看「哪条配置坏了」的手段。失败范围也**只限那一处**：好的来源照常返回。
/// 某个技能被**哪些地方**声明（阶段级 / 节点级），去重且保序。
///
/// 收在 core 里是因为有两个消费方：`GET /skills` 的 `declared_in` 字段与值班长的
/// `read_skills` 工具（票 01）。两处各写一遍这个循环，迟早一处改了另一处不改——
/// 而「这个技能被谁用着」正是回答「能不能删掉它 / 为什么它没生效」的关键读数。
pub fn declared_skill_where(stage_configs: &[StageConfig], name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cfg in stage_configs {
        for (where_, decl) in declared_skill_decls(cfg) {
            if decl.name == name && !out.contains(&where_) {
                out.push(where_);
            }
        }
    }
    out
}

/// 某个技能是否被**指定的这一个阶段**声明（阶段级或该阶段的任一节点级）。
///
/// 与 [`declared_skill_where`] 是同一个判定的两种形态：那一支回答「被谁用着」，回的是给人看的
/// 位置说明；这一支回答「这个阶段用没用它」，回一个布尔值——推荐面板据此决定那一行给的是
/// 「安装」还是「启用」（票 16「已安装的可直接启用」，票 01）。
///
/// **为什么不拿 [`declared_skill_where`] 的结果去比对字符串**：那等于让调用方 parse 展示文案
/// （`阶段 <key>` / `阶段 <key> 节点 <node>`），文案一改判定就悄悄失效。两支同源，都走
/// [`declared_skill_decls`]，故一条坏的节点声明一样只作废那一处来源。
pub fn skill_declared_in_stage(
    stage_configs: &[StageConfig],
    stage: crate::types::Stage,
    name: &str,
) -> bool {
    stage_configs
        .iter()
        .filter(|cfg| cfg.stage == stage.as_str())
        .flat_map(declared_skill_decls)
        .any(|(_, decl)| decl.name == name)
}

/// 一个阶段配置声明的全部技能，带**声明位置**（阶段级 / 哪个节点级）。
pub fn declared_skill_decls(cfg: &StageConfig) -> Vec<(String, crate::agent::skills::SkillDecl)> {
    declared_skill_sources(cfg)
        .into_iter()
        .filter_map(|(where_, decls)| decls.ok().map(|d| (where_, d)))
        .flat_map(|(where_, decls)| {
            decls
                .into_iter()
                .map(move |d| (where_.clone(), d))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// [`parse_skill_decls`] 的空安全包装：字段缺席按空数组处理。
fn parse_skill_decls_opt(
    value: Option<&serde_json::Value>,
    where_: &str,
) -> Result<Vec<crate::agent::skills::SkillDecl>> {
    match value {
        Some(v) => parse_skill_decls(v, where_),
        None => Ok(Vec::new()),
    }
}

/// 阶段配置声明的工具名（`tools_json`，决策 154 的后续票）。
///
/// **与 [`parse_skill_decls`] 的宽松口径刻意不同**：技能那一侧的「非字符串元素按不是声明
/// 忽略」是历史兼容（旧版单字符串形态），而工具名没有这层包袱——`tools_json` 一直是字符串数组。
/// 写了不是字符串的东西（`42` / `null` / `{"name": "read_file"}`）在这里就是**配置错误**：
/// 容忍它等于给「配置写了却没生效」留一条静默路径，而那正是本票要关掉的东西。
///
/// `where_` 把报错定位到阶段（照 [`parse_skill_decls`] 的报错风格）。
pub fn parse_tool_names(value: Option<&serde_json::Value>, where_: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| Error::Config(format!("{where_} 的 tools_json 不是数组：{value}")))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let name = item.as_str().ok_or_else(|| {
            Error::Config(format!(
                "{where_} 的 tools_json 里有非字符串项：{item}（工具名必须是字符串）"
            ))
        })?;
        out.push(name.to_string());
    }
    Ok(out)
}

/// 启动校验的输入（provider / 阶段配置来自 DB）。
#[derive(Debug, Clone, Default)]
pub struct StartupInputs {
    pub settings: Settings,
    pub providers: Vec<Provider>,
    pub stage_configs: Vec<StageConfig>,
    /// 本机可用的 skill 名（配置里引用不存在的 skill → fail fast）。
    pub available_skills: Vec<String>,
    /// home 根目录（§10.6.4：`persona_path` 相对路径按它解析；
    /// 提供时校验 persona_path 存在且非空，None 则跳过该文件系统检查）。
    pub home_root: Option<std::path::PathBuf>,
    /// 技能根目录（决策 170 / 172：默认 `{home}/skills`，可由 `[skills] dir` 覆盖；
    /// 提供时校验知识型技能正文存在且非空、frontmatter `name` 与目录名一致，
    /// `None` 则跳过该文件系统检查——老调用点无需改动）。
    pub skills_root: Option<std::path::PathBuf>,
}

/// 启动校验结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupReport {
    /// 因供应商不受支持而降级 `enabled = 0` 的 provider id（决策 103：降级不崩溃）。
    pub demoted_providers: Vec<String>,
}

/// 启动校验（§6「配置 fail fast」）：
/// - `cross_family_judge = true` 但 `validator_cross_check` 伪阶段没有可用 provider → 拒绝启动；
/// - 阶段引用的 provider 不存在 / 被禁用 / vendor 不受支持 → 拒绝启动；
/// - 引用的 skill 不存在 → 拒绝启动（阶段级与节点级都校验，节点级报错指明节点，决策 170）；
/// - `tools_json` 声明了 v1 不存在的工具名 → 拒绝启动（决策 154 的后续票；
///   形态非法——非数组 / 非字符串项——同样拒绝，见 [`parse_tool_names`]）；
/// - 提供 `home_root` 时：阶段 `persona_path` 不可读或内容为空 → 拒绝启动（§10.6.4）；
/// - 提供 `skills_root` 时：知识型技能的正文不可读或为空 → 拒绝启动（决策 170），
///   且 frontmatter `name` 与目录名不一致 → 拒绝启动（决策 172）；
/// - provider 表里 vendor 不受支持但**未被引用** → 降级 `enabled = 0`，只报告不报错。
pub fn validate_startup(inputs: &StartupInputs) -> Result<StartupReport> {
    let mut report = StartupReport::default();
    let mut usable: Vec<&Provider> = Vec::new();

    // frontmatter `name` 必须与目录名一致（决策 172：名字是唯一身份，不被 frontmatter
    // 悄悄覆盖）。与技能正文的检查同口径——提供技能根才做文件系统校验。
    if let Some(root) = &inputs.skills_root {
        crate::agent::skills::validate_names(root)?;
    }

    for p in &inputs.providers {
        if SUPPORTED_ADAPTERS.contains(&p.vendor.as_str()) {
            usable.push(p);
        } else {
            report.demoted_providers.push(p.id.clone());
        }
    }

    if inputs.settings.cross_family_judge {
        let cross = inputs
            .stage_configs
            .iter()
            .find(|c| c.stage == "validator_cross_check")
            .ok_or_else(|| {
                Error::Config(
                    "cross_family_judge = true，但未注册 validator_cross_check 伪阶段配置".into(),
                )
            })?;
        let provider_id = cross.provider_id.as_deref().ok_or_else(|| {
            Error::Config(
                "cross_family_judge = true，但 validator_cross_check 未配置 provider".into(),
            )
        })?;
        if !usable.iter().any(|p| p.id == provider_id && p.enabled) {
            return Err(Error::Config(format!(
                "cross_family_judge = true，但 validator_cross_check 的 provider {provider_id} 不可用"
            )));
        }
    }

    for cfg in &inputs.stage_configs {
        if let Some(pid) = cfg.provider_id.as_deref() {
            let provider = usable.iter().find(|p| p.id == pid).ok_or_else(|| {
                Error::Config(format!(
                    "阶段 {} 引用了不存在或不受支持的 provider：{pid}",
                    cfg.stage
                ))
            })?;
            if !provider.enabled {
                return Err(Error::Config(format!(
                    "阶段 {} 引用的 provider {pid} 已被禁用",
                    cfg.stage
                )));
            }
        }
        // 技能校验（决策 47 / 170 / 172④ / 185）：阶段级与节点级声明过同一套检查——
        // ① 声明形态合法（未知 mode / 未信任 + full → 拒绝，见 parse_skill_decls）；
        // ② 名字必须存在于可用技能集；③ 全文态技能的正文必须存在且非空。
        //
        // 名字的判定**只看技能根**（决策 185）：PATH 里有没有同名可执行文件不影响结果——
        // 二进制不再是一种技能。报错因此要把这一点说出来，否则从旧版本升上来的配置会看到
        // 一句「不存在的 skill」而以为文件丢了。
        for (where_, decl) in declared_skills(cfg)? {
            let skill = &decl.name;
            if !inputs.available_skills.iter().any(|s| s == skill) {
                return Err(Error::Config(format!(
                    "{where_} 引用了不存在的 skill：{skill}（技能只有 markdown 一个来源，\
                     位于技能根下的 {{name}}/SKILL.md；PATH 里的可执行文件不算技能，\
                     要用它请让 agent 经 run_command 调用）"
                )));
            }
            // 提供技能根时校验正文。名字态不读正文（票 05）——正文由 `Skill` 工具按需拉取，
            // 此处不校验其存在性。
            if let Some(root) = &inputs.skills_root {
                if decl.mode == crate::agent::skills::SkillMode::Full {
                    crate::agent::skills::resolve(root, std::slice::from_ref(&decl)).map_err(
                        |e| Error::Config(format!("{where_} 的 skill {skill} 不可用：{e}")),
                    )?;
                }
            }
        }
        // 工具名校验（决策 154 的后续票，姿态与「引用不存在的 skill」同层同源）：
        // `tools_json` 里出现 v1 不存在的名字 → **拒绝**，不再「静默丢弃 + 一条 warn」。
        // 触发场景是前端那格自由文本：拼错的名字在旧行为下只留一行日志，「配了却没生效」
        // 因此无从察觉（与 `deny_unknown_fields` / 决策 134 的 fail fast 姿态对齐）。
        //
        // 存量配置（库里已有的行）走同一条路：启动即报错并指明是哪个阶段的哪个名字，
        // **不静默放行、也不自动清理**——自动清理会把用户的错字悄悄抹掉，让人再也看不到
        // 自己写错了什么。要放行就改那一行配置（`PUT /stage-configs` 会给出同样的报文）。
        let where_ = format!("阶段 {} 的 tools_json", cfg.stage);
        let mut unknown: Vec<String> = Vec::new();
        for name in parse_tool_names(cfg.tools_json.as_ref(), &where_)? {
            if !crate::agent::client::is_known_tool_name(&name) && !unknown.contains(&name) {
                unknown.push(name);
            }
        }
        if !unknown.is_empty() {
            // 报文与执行期兜底同源（`client::unknown_tools_message`）：同一个错误一种说法
            return Err(Error::Config(crate::agent::client::unknown_tools_message(
                &where_, &unknown,
            )));
        }
        // §10.6.4：persona「必须存在且非空」在启动时校验（运行时 resolve_stage_persona
        // 仍有同样检查兜底——手工改库可绕过启动校验）
        if let (Some(root), Some(path)) = (&inputs.home_root, cfg.persona_path.as_deref()) {
            let p = root.join(path);
            let content = std::fs::read_to_string(&p).map_err(|e| {
                Error::Config(format!(
                    "阶段 {} 的 persona_path 不可读：{}（{e}）",
                    cfg.stage,
                    p.display()
                ))
            })?;
            if content.trim().is_empty() {
                return Err(Error::Config(format!(
                    "阶段 {} 的 persona_path 内容为空：{}",
                    cfg.stage,
                    p.display()
                )));
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Stage, ENV_MODE_EXPECTED};

    #[test]
    fn defaults_match_design_table() {
        let s = Settings::default();
        assert_eq!(s.validate_retry_max, 3);
        assert_eq!(s.agent_retry_max, 3);
        assert_eq!(s.tool_retry_max, 3);
        assert_eq!(s.node_idle_timeout_sec, 300);
        assert_eq!(s.node_max_duration_sec, 1800);
        assert_eq!(s.tool_timeout_sec, 60);
        assert_eq!(s.test_command_timeout_sec, 600);
        assert!(!s.adaptive_timeout_enabled);
        assert_eq!(s.pending_resume_cooldown_sec, 5);
        assert_eq!(s.pending_reminder_hours, 24);
        assert_eq!(s.pending_timeout_hours, 72);
        assert_eq!(s.tick_interval_sec, 10);
        assert_eq!(s.conversation_max_chars, 200_000);
        assert_eq!(s.conversation_retention_days, 30);
        assert_eq!(s.offload_threshold_tokens, 4000);
        assert_eq!(s.context_soft_limit_ratio, 0.6);
        assert_eq!(s.keep_recent_rounds, 5);
        assert_eq!(s.context_hard_limit_ratio, 0.9);
        assert!(s.semantic_conflict_check);
        assert!(!s.cross_family_judge);
        assert_eq!(s.conflict_overlap_threshold, 0);
        assert_eq!(s.max_concurrent_tasks, 5);
        assert!(!s.allow_dirty_worktree_merge);
    }

    /// 默认端口是「唯一事实源」（决策 171）：缺省绑定与跨源白名单的两个本机 origin
    /// 都由它派生，改动必须在此显式反映，否则局域网 / 桌面壳的放行集合会静默漂移。
    #[test]
    fn default_server_port_is_8788() {
        let server = ServerConfig::default();
        assert_eq!(server.port, 8788);
        assert_eq!(server.host, "127.0.0.1");
    }

    /// `[pipeline] env_mode` 走通两层解析，且**只收 `auto` / `deny`**（决策 206）。
    ///
    /// `ask` 在这一层是「所有阶段的全局默认」，而流水线节点无人按那颗确认钮、也没有提议通道
    /// ——配成 `ask` 的结果是**静默收掉全机所有环境写动作**（一条 develop 会卡在「写不了文件」
    /// 上，而配置看上去只是一行 `ask`）。故它在解析期就被拒，报错指向真正该配它的那一行
    /// （`stage_configs` 的 `foreman`）。写错档位名同样 fail fast（方向只能是收紧，不能靠猜）。
    #[test]
    fn the_global_tier_accepts_auto_and_deny_but_not_ask() {
        for mode in ["auto", "deny"] {
            let cfg = Config::from_toml(&format!("[pipeline]\nenv_mode = \"{mode}\"\n")).unwrap();
            assert_eq!(cfg.settings().env_mode.as_str(), mode);
        }
        // `ask` 被拒，且报文说清「配在哪一行」
        let err = Config::from_toml("[pipeline]\nenv_mode = \"ask\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("不能是 ask"), "{msg}");
        assert!(msg.contains("deny"), "要说清该配什么：{msg}");
        assert!(msg.contains("foreman"), "要指出该配在哪一行：{msg}");
        // 认不出的值照旧 fail fast，且用共享那句话（与 `PUT /stage-configs` 同一份说法）
        let err = Config::from_toml("[pipeline]\nenv_mode = \"Auto\"\n").unwrap_err();
        assert!(err.to_string().contains(ENV_MODE_EXPECTED), "{err}");
    }

    #[test]
    fn toml_override_only_touches_named_fields() {
        let cfg = Config::from_toml(
            r#"
            [server]
            port = 9999

            [pipeline]
            max_concurrent_tasks = 1
            node_idle_timeout_sec = 10
            "#,
        )
        .unwrap();
        let s = cfg.settings();
        assert_eq!(cfg.server.port, 9999);
        assert_eq!(s.max_concurrent_tasks, 1);
        assert_eq!(s.node_idle_timeout_sec, 10);
        // 其余保持默认
        assert_eq!(s.validate_retry_max, 3);
        assert_eq!(s.test_command_timeout_sec, 600);
    }

    // ── 票 16：`[logging]` 键名与文档一致（format / file）──

    #[test]
    fn logging_keys_match_docs_format_and_file() {
        let cfg = Config::from_toml(
            r#"
            [logging]
            level = "debug"
            format = "json"
            file = "logs/agentpipeline.log"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.logging.level, "debug");
        assert_eq!(cfg.logging.format, Some(LogFormat::Json));
        assert_eq!(cfg.logging.file.as_deref(), Some("logs/agentpipeline.log"));
        assert_eq!(cfg.logging.effective_format(), LogFormat::Json);
    }

    #[test]
    fn logging_defaults_to_pretty_without_file() {
        let cfg = Config::from_toml("[logging]\nlevel = \"warn\"\n").unwrap();
        assert_eq!(cfg.logging.effective_format(), LogFormat::Pretty);
        assert!(cfg
            .logging
            .resolved_file(Path::new("/home/u/.agentpipeline"))
            .is_none());
        // 缺省不再默认 json（旧结构体 json_file = true 的语义不再无条件继承）
        assert_eq!(LoggingConfig::default().level, "info");
    }

    #[test]
    fn logging_unknown_key_is_rejected_not_silently_ignored() {
        // 姿态明确：未知键 fail fast（沿用决策 47 / 103 / 134）
        let err = Config::from_toml("[logging]\nlevel = \"info\"\nbogus = 1\n").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
        let err = Config::from_toml("[logging]\njson_fil = true\n").unwrap_err();
        assert!(err.to_string().contains("解析失败"), "{err}");
    }

    #[test]
    fn logging_deprecated_json_file_still_parses_and_maps_to_json() {
        // 旧键兼容：json_file = true ≡ format = "json"（废弃但可读）
        let cfg = Config::from_toml("[logging]\njson_file = true\n").unwrap();
        assert_eq!(cfg.logging.effective_format(), LogFormat::Json);
        let cfg = Config::from_toml("[logging]\njson_file = false\n").unwrap();
        assert_eq!(cfg.logging.effective_format(), LogFormat::Pretty);
    }

    #[test]
    fn logging_format_and_deprecated_json_file_conflict_fails_fast() {
        let err =
            Config::from_toml("[logging]\nformat = \"compact\"\njson_file = true\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("json_file"), "{msg}");
        assert!(msg.contains("不能同时配置"), "{msg}");
    }

    #[test]
    fn logging_file_path_resolution_expands_tilde_and_relative() {
        let home = Path::new("/home/u/.agentpipeline");

        let cfg = Config::from_toml("[logging]\nfile = \"/var/log/ap.log\"\n").unwrap();
        assert_eq!(
            cfg.logging.resolved_file(home).unwrap(),
            PathBuf::from("/var/log/ap.log")
        );

        // 相对路径按 home 根解析
        let cfg = Config::from_toml("[logging]\nfile = \"logs/ap.log\"\n").unwrap();
        assert_eq!(
            cfg.logging.resolved_file(home).unwrap(),
            home.join("logs/ap.log")
        );

        // 空白 = 不落文件
        let cfg = Config::from_toml("[logging]\nfile = \"   \"\n").unwrap();
        assert!(cfg.logging.resolved_file(home).is_none());
    }

    #[test]
    fn prompts_dir_resolution_matches_home_semantics() {
        let home = Path::new("/home/u/.agentpipeline");

        let cfg = Config::from_toml("[prompts]\ndir = \"/custom/prompts\"\n").unwrap();
        assert_eq!(
            cfg.prompts.resolved_dir(home).unwrap(),
            PathBuf::from("/custom/prompts")
        );

        let cfg = Config::from_toml("[prompts]\ndir = \"my-prompts\"\n").unwrap();
        assert_eq!(
            cfg.prompts.resolved_dir(home).unwrap(),
            home.join("my-prompts")
        );

        // 未配置 / 空白 → None（Home 回落 {home}/prompts）
        assert!(Config::from_toml("")
            .unwrap()
            .prompts
            .resolved_dir(home)
            .is_none());
        let cfg = Config::from_toml("[prompts]\ndir = \"  \"\n").unwrap();
        assert!(cfg.prompts.resolved_dir(home).is_none());
    }

    #[test]
    fn prompts_unknown_key_is_rejected() {
        let err = Config::from_toml("[prompts]\ndir = \"x\"\nnope = 1\n").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
    }

    // ── 决策 172：[skills] dir 覆盖技能根 ──

    #[test]
    fn skills_dir_resolution_matches_prompts_semantics() {
        let home = Path::new("/home/u/.agentpipeline");

        // 绝对路径原样
        let cfg = Config::from_toml("[skills]\ndir = \"/opt/shared/skills\"\n").unwrap();
        assert_eq!(
            cfg.skills.resolved_dir(home).unwrap(),
            PathBuf::from("/opt/shared/skills")
        );

        // 相对路径接在 home 下
        let cfg = Config::from_toml("[skills]\ndir = \"my-skills\"\n").unwrap();
        assert_eq!(
            cfg.skills.resolved_dir(home).unwrap(),
            home.join("my-skills")
        );

        // 未配置 / 空白 → None（Home 回落 {home}/skills，行为逐字不变）
        assert!(Config::from_toml("")
            .unwrap()
            .skills
            .resolved_dir(home)
            .is_none());
        let cfg = Config::from_toml("[skills]\ndir = \"   \"\n").unwrap();
        assert!(cfg.skills.resolved_dir(home).is_none());
    }

    #[test]
    fn skills_dir_expands_tilde() {
        // `~/.zcode/skills` 是这一项的主要用法（决策 172）
        let cfg = Config::from_toml("[skills]\ndir = \"~/.zcode/skills\"\n").unwrap();
        let resolved = cfg
            .skills
            .resolved_dir(Path::new("/home/u/.agentpipeline"))
            .unwrap();
        assert!(
            resolved.is_absolute() && resolved.ends_with(".zcode/skills"),
            "{}",
            resolved.display()
        );
    }

    #[test]
    fn skills_unknown_key_is_rejected() {
        let err = Config::from_toml("[skills]\ndir = \"x\"\nnope = 1\n").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
    }

    // ── 决策 194：`[market] github_repos` 来源仓名单（取代决策 172⑤ 的 origin 白名单）──

    /// **默认空 = 不从任何仓安装**（保守方向上的默认，票面显式要求）。
    #[test]
    fn market_defaults_to_no_allowed_repo() {
        let cfg = Config::from_toml("").unwrap();
        assert!(cfg.market.github_repos.is_empty());
        assert!(cfg.market.resolved_repos().is_empty());
    }

    /// 写入的仓名被归一（去粘贴残留），重复项按大小写不敏感去重且**保留顺序**。
    #[test]
    fn market_repos_are_normalized_deduped_and_ordered() {
        let cfg = Config::from_toml(
            "[market]\ngithub_repos = [\"Obra/Superpowers\", \"mattpocock/skills\", \"https://github.com/obra/superpowers.git\"]\n",
        )
        .unwrap();
        assert_eq!(
            cfg.market.resolved_repos(),
            vec![
                "Obra/Superpowers".to_string(),
                "mattpocock/skills".to_string(),
            ]
        );
    }

    /// 非法仓名 fail fast，不静默丢弃——丢一条的表现是「我明明配了它却说没放行」。
    /// 带 scheme / 带路径 / 带 `@` 的写法全拒（URL 由程序拼，输入不许决定走哪条 transport）。
    #[test]
    fn market_rejects_illegal_repo_names() {
        for bad in [
            "[market]\ngithub_repos = [\"https://gitlab.com/obra/superpowers\"]\n",
            "[market]\ngithub_repos = [\"ssh://git@github.com/obra/x.git\"]\n",
            "[market]\ngithub_repos = [\"git@github.com:obra/x.git\"]\n",
            "[market]\ngithub_repos = [\"obra\"]\n",
            "[market]\ngithub_repos = [\"obra/x/extra\"]\n",
            "[market]\ngithub_repos = [\"../etc/passwd\"]\n",
            "[market]\ngithub_repos = [\"\"]\n",
        ] {
            let err = Config::from_toml(bad).unwrap_err();
            assert!(matches!(err, Error::Config(_)), "{bad} → {err}");
            assert!(err.to_string().contains("github_repos"), "{bad} → {err}");
        }
    }

    /// 旧键**报一句能照着改的话**，而不是「unknown field」。
    /// 这一页是唯一的安全控制，配错了却以为配上了是最坏的情形。
    #[test]
    fn market_legacy_allowed_sources_key_says_what_replaced_it() {
        let err =
            Config::from_toml("[market]\nallowed_sources = [\"https://skills.example.com\"]\n")
                .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, Error::Config(_)), "{msg}");
        assert!(msg.contains("github_repos"), "须给出替代键：{msg}");
        assert!(msg.contains("194"), "须点明是哪条决策改的：{msg}");
        assert!(msg.contains("skills.example.com"), "须回显旧值：{msg}");
    }

    #[test]
    fn market_unknown_key_is_rejected() {
        let err = Config::from_toml("[market]\ngithub_repos = []\nnope = 1\n").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
    }

    #[test]
    fn unknown_top_level_and_section_keys_are_rejected() {
        // 未知 section 与未知 pipeline 键都不静默忽略
        assert!(Config::from_toml("[mystery]\nx = 1\n").is_err());
        assert!(Config::from_toml("[pipeline]\nnot_a_key = 1\n").is_err());
        assert!(Config::from_toml("[server]\nport = 1\nbogus = true\n").is_err());
    }

    // ── 决策 157：[server] allowed_origins ──

    #[test]
    fn allowed_origins_parse_and_normalize() {
        let cfg = Config::from_toml(
            r#"
            [server]
            allowed_origins = ["HTTP://192.168.1.10:8787/", "https://ap.example.local"]
            "#,
        )
        .unwrap();
        // 原值按用户写法保留；归一（小写化 / 剥尾斜杠）发生在 serve 注入前
        assert_eq!(
            cfg.server.allowed_origins,
            vec!["HTTP://192.168.1.10:8787/", "https://ap.example.local"]
        );
        assert_eq!(
            normalize_origin("HTTP://192.168.1.10:8787/").unwrap(),
            "http://192.168.1.10:8787"
        );
        assert_eq!(
            normalize_origin("  https://AP.Example.local  ").unwrap(),
            "https://ap.example.local"
        );
    }

    #[test]
    fn allowed_origins_invalid_values_fail_fast() {
        for bad in [
            "ftp://192.168.1.10:8787", // scheme 不支持
            "192.168.1.10:8787",       // 缺 scheme
            "http://host/path",        // 带路径
        ] {
            let toml = format!("[server]\nallowed_origins = [\"{bad}\"]\n");
            assert!(
                Config::from_toml(&toml).is_err(),
                "非法 origin 应 fail fast：{bad}"
            );
        }
    }

    #[test]
    fn effective_timeout_layering_node_beats_stage_beats_global() {
        let global = 300;
        assert_eq!(
            effective_idle_timeout(global, None, NodeTimeouts::default()),
            300
        );
        assert_eq!(
            effective_idle_timeout(global, Some(120), NodeTimeouts::default()),
            120
        );
        assert_eq!(
            effective_idle_timeout(
                global,
                Some(120),
                NodeTimeouts {
                    idle_timeout_sec: Some(45),
                    max_duration_sec: None
                }
            ),
            45
        );
        assert_eq!(
            effective_max_duration(1800, None, NodeTimeouts::default()),
            1800
        );
        assert_eq!(
            effective_max_duration(
                1800,
                Some(900),
                NodeTimeouts {
                    idle_timeout_sec: None,
                    max_duration_sec: Some(60)
                }
            ),
            60
        );
    }

    #[test]
    fn node_overrides_json_parsed() {
        let mut cfg = StageConfig {
            stage: "merge".into(),
            ..Default::default()
        };
        cfg.node_overrides_json = Some(serde_json::json!({
            "execute": { "idle_timeout_sec": 600, "max_duration_sec": 900 }
        }));
        let t = node_timeouts(Some(&cfg), "execute");
        assert_eq!(t.idle_timeout_sec, Some(600));
        assert_eq!(t.max_duration_sec, Some(900));
        assert_eq!(
            node_timeouts(Some(&cfg), "validate_input"),
            NodeTimeouts::default()
        );
        assert_eq!(node_timeouts(None, "execute"), NodeTimeouts::default());
    }

    #[test]
    fn run_command_timeout_depends_on_stage() {
        let s = Settings::default();
        assert_eq!(effective_run_command_timeout(&s, Stage::Test, None), 600);
        assert_eq!(effective_run_command_timeout(&s, Stage::Merge, None), 600);
        assert_eq!(effective_run_command_timeout(&s, Stage::Develop, None), 60);
        assert_eq!(
            effective_run_command_timeout(&s, Stage::Develop, Some(120)),
            120
        );
    }

    fn provider(id: &str, vendor: &str, enabled: bool) -> Provider {
        Provider {
            id: id.into(),
            vendor: vendor.into(),
            model: "m".into(),
            context_window: 1000,
            base_url: None,
            api_key: None,
            enabled,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn cross_family_judge_without_provider_fails_fast() {
        let inputs = StartupInputs {
            settings: Settings {
                cross_family_judge: true,
                ..Default::default()
            },
            providers: vec![provider("p1", "deepseek", true)],
            stage_configs: vec![],
            available_skills: vec![],
            home_root: None,
            skills_root: None,
        };
        let err = validate_startup(&inputs).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn unsupported_vendor_demoted_but_not_fatal() {
        let inputs = StartupInputs {
            providers: vec![provider("p1", "mystery-llm", true)],
            ..Default::default()
        };
        let report = validate_startup(&inputs).unwrap();
        assert_eq!(report.demoted_providers, vec!["p1".to_string()]);
    }

    #[test]
    fn referenced_unsupported_vendor_fails_fast() {
        let inputs = StartupInputs {
            providers: vec![provider("p1", "mystery-llm", true)],
            stage_configs: vec![StageConfig {
                stage: "develop".into(),
                provider_id: Some("p1".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(validate_startup(&inputs).is_err());
    }

    #[test]
    fn node_skills_reads_per_node_declaration() {
        // 决策 170：节点级技能声明（`node_overrides_json[node].skills`）
        use crate::agent::skills::{SkillDecl, SkillMode};
        let cfg = StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["grilling"]},
                "execute": {"skills": ["to-spec"]},
                "validate_output": {"idle_timeout_sec": 60}
            })),
            ..Default::default()
        };
        // 裸字符串按 `{mode: full, trusted: false}` 解释（决策 172④：零迁移）
        assert_eq!(
            node_skills(Some(&cfg), "validate_input").unwrap(),
            vec![SkillDecl::from_bare("grilling")]
        );
        assert_eq!(
            node_skills(Some(&cfg), "execute").unwrap(),
            vec![SkillDecl {
                name: "to-spec".into(),
                mode: SkillMode::Full,
                trusted: false
            }]
        );
        // 只配了超时的节点没有技能
        assert!(node_skills(Some(&cfg), "validate_output")
            .unwrap()
            .is_empty());
        // 未声明的节点 / 无配置 / 无 stage_cfg 一律空（只增不减，不报错）
        assert!(node_skills(Some(&cfg), "init").unwrap().is_empty());
        assert!(node_skills(None, "execute").unwrap().is_empty());
        let empty = StageConfig {
            stage: "develop".into(),
            ..Default::default()
        };
        assert!(node_skills(Some(&empty), "execute").unwrap().is_empty());
    }

    /// `declared_skill_decls`（只读界面路径）只丢掉**解析失败的那一处来源**。
    ///
    /// 早先它是对整份配置 `unwrap_or_default()`：一条坏的节点声明会让同阶段**其他节点**的技能
    /// 也报成「无人引用」，恰好在「卸载前看后果」这个用途上给出错误的安全感。
    #[test]
    fn declared_skill_decls_drops_only_the_broken_source() {
        let cfg = StageConfig {
            stage: "architect-design".into(),
            // 阶段级合法
            skills_json: Some(serde_json::json!(["stage-skill"])),
            node_overrides_json: Some(serde_json::json!({
                // 坏：未受信任却要 full 注入
                "validate_input": {"skills": [{"name": "bad", "mode": "full", "trusted": false}]},
                // 好：同配置里的另一节点
                "execute": {"skills": ["node-skill"]},
            })),
            ..Default::default()
        };
        let decls = declared_skill_decls(&cfg);
        let names: Vec<&str> = decls.iter().map(|(_, d)| d.name.as_str()).collect();
        assert!(
            names.contains(&"stage-skill"),
            "阶段级好声明不该被节点的坏声明连累：{names:?}"
        );
        assert!(
            names.contains(&"node-skill"),
            "同配置另一节点的好声明不该被连累：{names:?}"
        );
        assert!(!names.contains(&"bad"), "{names:?}");

        // 而写入 / 启动路径仍 fail fast（同一份配置经严格版即报错）
        assert!(declared_skills(&cfg).is_err());
    }

    /// 推荐面板的「启用」判据：按**阶段**问「这个技能在这个阶段启用没有」。
    ///
    /// 关键的一格是「同名技能在别的阶段被声明 ≠ 本阶段被声明」——推荐清单里 `tdd` 同时挂在
    /// test-design 与 develop 两行上，而它只被 test-design 声明过（票 01 要分辨的正是这一格）。
    #[test]
    fn skill_declared_in_stage_is_per_stage_and_counts_node_level() {
        let cfgs = vec![
            StageConfig {
                stage: "architect-design".into(),
                skills_json: Some(serde_json::json!(["grilling"])),
                node_overrides_json: Some(serde_json::json!({
                    "execute": {"skills": ["to-spec"]},
                })),
                ..Default::default()
            },
            StageConfig {
                stage: "develop".into(),
                skills_json: Some(serde_json::json!(["tdd"])),
                ..Default::default()
            },
        ];

        // 阶段级声明
        assert!(skill_declared_in_stage(
            &cfgs,
            Stage::ArchitectDesign,
            "grilling"
        ));
        // 节点级声明**也算**这个阶段声明了它（推荐面板那一行同样不该再给「启用」）
        assert!(skill_declared_in_stage(
            &cfgs,
            Stage::ArchitectDesign,
            "to-spec"
        ));
        // 本阶段没有它，别的阶段有 —— 两行推荐各自独立回答
        assert!(skill_declared_in_stage(&cfgs, Stage::Develop, "tdd"));
        assert!(!skill_declared_in_stage(
            &cfgs,
            Stage::ArchitectDesign,
            "tdd"
        ));
        assert!(!skill_declared_in_stage(&cfgs, Stage::Develop, "grilling"));
        // 一个字都没声明过的技能、以及没有配置行的阶段：false 而不是报错
        assert!(!skill_declared_in_stage(&cfgs, Stage::Review, "tdd"));
        assert!(!skill_declared_in_stage(&cfgs, Stage::Merge, "tdd"));

        // 坏的那一处来源只作废它自己：同配置里的好声明照常算已声明
        let broken = StageConfig {
            stage: "review".into(),
            skills_json: Some(serde_json::json!(["code-review"])),
            node_overrides_json: Some(serde_json::json!({
                "execute": {"skills": [{"name": "bad", "mode": "full", "trusted": false}]},
            })),
            ..Default::default()
        };
        assert!(skill_declared_in_stage(
            &[broken],
            Stage::Review,
            "code-review"
        ));
    }

    #[test]
    fn node_skills_ignores_non_array_and_non_string_entries() {
        let cfg = StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": "grilling"},
                "execute": {"skills": ["to-spec", 42, null]}
            })),
            ..Default::default()
        };
        assert!(node_skills(Some(&cfg), "validate_input")
            .unwrap()
            .is_empty());
        assert_eq!(
            node_skills(Some(&cfg), "execute").unwrap(),
            vec![crate::agent::skills::SkillDecl::from_bare("to-spec")],
            "杂值不是声明，忽略（票 01 宽松口径不变）"
        );
    }

    // ── 票 05（决策 172④）：混合数组解析与信任门 ──

    #[test]
    fn bare_strings_are_backward_compatible_full_untrusted() {
        // 旧配置行（纯字符串数组）行为逐字不变：零迁移
        let decls = parse_skill_decls(&serde_json::json!(["a", "b"]), "阶段 develop").unwrap();
        assert_eq!(
            decls,
            vec![
                crate::agent::skills::SkillDecl::from_bare("a"),
                crate::agent::skills::SkillDecl::from_bare("b"),
            ]
        );
    }

    #[test]
    fn object_form_reads_mode_and_trusted() {
        use crate::agent::skills::SkillMode;
        let decls = parse_skill_decls(
            &serde_json::json!([
                {"name": "a", "mode": "name", "trusted": true},
                {"name": "b", "mode": "name"},
                {"name": "c", "trusted": true}
            ]),
            "阶段 develop",
        )
        .unwrap();
        assert_eq!(decls[0].mode, SkillMode::Name);
        assert!(decls[0].trusted);
        assert_eq!(decls[1].mode, SkillMode::Name);
        assert!(!decls[1].trusted);
        // mode 缺省 full；trusted 显式为 true 时允许全文注入
        assert_eq!(decls[2].mode, SkillMode::Full);
        assert!(decls[2].trusted);
        assert_eq!(decls[2].name, "c");
    }

    /// 信任门只对**显式对象**生效：`{"name": "d"}`（full + trusted 缺省 false）被拒绝，
    /// 而同样的技能写成裸字符串 `"d"` 则放行。
    ///
    /// 这不是漏洞而是「零迁移」的必要条件：信任概念出现之前手写的配置行全是裸字符串，
    /// 若把信任门一视同仁地施加于它们，升级后今天所有配置行都会失效。裸字符串因此被当作
    /// 「用户在自己机器上手写的既有配置」，与「刚从市场装来、尚未信任」的新对象分开对待。
    #[test]
    fn trust_gate_applies_to_object_form_only() {
        use crate::agent::skills::SkillDecl;
        // 显式对象 + full + 未信任 → 拒绝
        let err =
            parse_skill_decls(&serde_json::json!([{"name": "d"}]), "阶段 develop").unwrap_err();
        assert!(
            err.to_string().contains("d") && err.to_string().contains("信任"),
            "{err}"
        );
        // 裸字符串同义声明 → 放行（旧配置行零迁移）
        assert_eq!(
            parse_skill_decls(&serde_json::json!(["d"]), "阶段 develop").unwrap(),
            vec![SkillDecl::from_bare("d")]
        );
    }

    #[test]
    fn invalid_mode_is_rejected_with_location() {
        let err = parse_skill_decls(
            &serde_json::json!([{"name": "a", "mode": "half"}]),
            "阶段 develop 节点 execute",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("half") && msg.contains("full") && msg.contains("name"),
            "{msg}"
        );
        assert!(msg.contains("节点 execute"), "报错须定位到节点：{msg}");
    }

    #[test]
    fn untrusted_full_injection_is_rejected() {
        // 选型 D：未信任技能不得全文注入（写入时拒绝）
        let err = parse_skill_decls(
            &serde_json::json!([{"name": "evil", "mode": "full", "trusted": false}]),
            "阶段 develop",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("evil") && msg.contains("信任"), "{msg}");

        // 未信任 + name 模式是允许的（正文由 Skill 工具按需拉取）
        assert!(parse_skill_decls(
            &serde_json::json!([{"name": "ok", "mode": "name", "trusted": false}]),
            "阶段 develop"
        )
        .is_ok());
        // 已信任 + full 也允许
        assert!(parse_skill_decls(
            &serde_json::json!([{"name": "ok", "mode": "full", "trusted": true}]),
            "阶段 develop"
        )
        .is_ok());
    }

    #[test]
    fn object_without_name_is_rejected() {
        let err =
            parse_skill_decls(&serde_json::json!([{"mode": "name"}]), "阶段 develop").unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    // ───────────────────────── 信任转换（票 11）─────────────────────────

    fn cfg_with_skills(skills: serde_json::Value) -> StageConfig {
        StageConfig {
            stage: "develop".into(),
            skills_json: Some(skills),
            ..Default::default()
        }
    }

    /// 显式对象：`trusted` 就地翻成 true，`mode` 与其他技能一概不动。
    #[test]
    fn trust_conversion_flips_explicit_object_in_place() {
        let cfg = cfg_with_skills(serde_json::json!([
            {"name": "a", "mode": "name", "trusted": false},
            {"name": "b", "mode": "full", "trusted": true}
        ]));
        let out = set_skill_trust(&cfg, "a", true).unwrap().expect("应改写");
        let decls = parse_skill_decls(out.skills_json.as_ref().unwrap(), "阶段 develop").unwrap();
        assert_eq!(decls[0].name, "a");
        assert!(decls[0].trusted, "a 应已信任");
        assert_eq!(
            decls[0].mode,
            crate::agent::skills::SkillMode::Name,
            "mode 不动"
        );
        // b 原样保留
        assert_eq!(decls[1].name, "b");
        assert!(decls[1].trusted);
    }

    /// 裸字符串物化为显式对象：`full` 语义不变，只是多了一个 `trusted: true` 的位置。
    ///
    /// 这是**语义等价**改写（裸字符串本就按 `{full, false}` 解释），不是降级。
    #[test]
    fn trust_conversion_materializes_bare_string_keeping_full_mode() {
        let cfg = cfg_with_skills(serde_json::json!(["grill", "other"]));
        let out = set_skill_trust(&cfg, "grill", true)
            .unwrap()
            .expect("应改写");
        let decls = parse_skill_decls(out.skills_json.as_ref().unwrap(), "阶段 develop").unwrap();
        assert_eq!(decls[0].name, "grill");
        assert!(decls[0].trusted);
        assert_eq!(decls[0].mode, crate::agent::skills::SkillMode::Full);
        // 另一个裸字符串原样留着（没被顺带规范化）
        assert_eq!(
            out.skills_json.as_ref().unwrap()[1],
            serde_json::json!("other")
        );
    }

    /// 未信任 + `full` 是禁止状态：降信任撞上它必须**拒绝**，而不是静默把 full 改成 name。
    #[test]
    fn untrusting_a_full_declaration_is_refused_not_silently_downgraded() {
        let cfg = cfg_with_skills(serde_json::json!([
            {"name": "a", "mode": "full", "trusted": true}
        ]));
        let err = set_skill_trust(&cfg, "a", false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("a") && msg.contains("full") && msg.contains("name"),
            "报文须指出该怎么改：{msg}"
        );
        // 原配置未被改动（拒绝时不产生半成品）
        assert!(cfg.skills_json.as_ref().unwrap()[0]["trusted"]
            .as_bool()
            .unwrap());
    }

    /// 降信任在名字态上是安全的：正文不进 prompt，改回未信任不产生非法配置。
    #[test]
    fn untrusting_a_name_mode_declaration_is_allowed() {
        let cfg = cfg_with_skills(serde_json::json!([
            {"name": "a", "mode": "name", "trusted": true}
        ]));
        let out = set_skill_trust(&cfg, "a", false).unwrap().expect("应改写");
        let decls = parse_skill_decls(out.skills_json.as_ref().unwrap(), "阶段 develop").unwrap();
        assert!(!decls[0].trusted);
        assert_eq!(decls[0].mode, crate::agent::skills::SkillMode::Name);
    }

    /// 没引用该技能 → 不改（`None`），避免为无关技能写盘。
    #[test]
    fn unrelated_skill_leaves_the_config_untouched() {
        let cfg = cfg_with_skills(serde_json::json!([{"name": "a", "trusted": false}]));
        assert!(set_skill_trust(&cfg, "other", true).unwrap().is_none());
    }

    /// 节点级声明与阶段级走同一套改写（票 05 的解析器两处共用，改写也不该分家）。
    #[test]
    fn trust_conversion_covers_node_level_declarations() {
        let cfg = StageConfig {
            stage: "architect-design".into(),
            skills_json: Some(serde_json::json!(["grill"])),
            node_overrides_json: Some(serde_json::json!({
                "execute": {"skills": [{"name": "grill", "mode": "name", "trusted": false}]},
                "validate_output": {"idle_timeout_sec": 60}
            })),
            ..Default::default()
        };
        let out = set_skill_trust(&cfg, "grill", true)
            .unwrap()
            .expect("应改写");
        // 阶段级裸字符串物化
        assert_eq!(
            out.skills_json.as_ref().unwrap()[0]["trusted"],
            serde_json::json!(true)
        );
        // 节点级同改
        assert_eq!(
            out.node_overrides_json.as_ref().unwrap()["execute"]["skills"][0]["trusted"],
            serde_json::json!(true)
        );
        // 无 skills 的节点的其他字段原样保留
        assert_eq!(
            out.node_overrides_json.as_ref().unwrap()["validate_output"]["idle_timeout_sec"],
            serde_json::json!(60)
        );
    }

    /// 反方向同一条门：转换出的配置必须**仍能通过**写入校验。
    ///
    /// 这条钉的是「转换后的产物合法」——只断言 JSON 形状不够，得让真正会守门的那位过一遍。
    #[test]
    fn converted_config_still_passes_the_write_gate() {
        let cfg = cfg_with_skills(serde_json::json!([
            {"name": "a", "mode": "name", "trusted": false}
        ]));
        // 只信任、不改 mode：name 态下信任与否都不影响合法性
        let out = set_skill_trust(&cfg, "a", true).unwrap().unwrap();
        assert!(parse_skill_decls(out.skills_json.as_ref().unwrap(), "阶段 develop").is_ok());
    }

    #[test]
    fn missing_skill_fails_fast() {
        let inputs = StartupInputs {
            providers: vec![provider("p1", "deepseek", true)],
            stage_configs: vec![StageConfig {
                stage: "develop".into(),
                provider_id: Some("p1".into()),
                skills_json: Some(serde_json::json!(["rtk"])),
                ..Default::default()
            }],
            available_skills: vec![],
            ..Default::default()
        };
        assert!(validate_startup(&inputs).is_err());
    }

    #[test]
    fn persona_path_checked_at_startup_when_home_root_given() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("persona.md");
        std::fs::write(&good, "有效 persona").unwrap();
        let base = StartupInputs {
            stage_configs: vec![StageConfig {
                stage: "review".into(),
                persona_path: Some("persona.md".into()),
                ..Default::default()
            }],
            ..Default::default()
        };

        // home_root 未提供 → 跳过文件系统检查
        assert!(validate_startup(&StartupInputs { ..base.clone() }).is_ok());

        // 文件有效 → 通过
        let inputs = StartupInputs {
            home_root: Some(tmp.path().to_path_buf()),
            ..base.clone()
        };
        assert!(validate_startup(&inputs).is_ok());

        // 文件为空 → fail fast（§10.6.4：persona 必须存在且非空）
        std::fs::write(&good, "  \n").unwrap();
        let err = validate_startup(&inputs).unwrap_err();
        assert!(err.to_string().contains("内容为空"), "{err}");

        // 文件缺失 → fail fast
        std::fs::remove_file(&good).unwrap();
        let err = validate_startup(&inputs).unwrap_err();
        assert!(err.to_string().contains("不可读"), "{err}");
    }

    /// 决策 154 的后续票：`tools_json` 里的**未知工具名拒绝**（静默丢弃 → fail fast）。
    ///
    /// 与「引用不存在的 skill」同层同源：判据是 `client::is_known_tool_name` 一处，
    /// 报文给出未知名字 + v1 已知工具集，让用户照着改。
    #[test]
    fn unknown_tool_names_fail_startup_validation() {
        let cfg = |tools: serde_json::Value| StartupInputs {
            stage_configs: vec![StageConfig {
                stage: "develop".into(),
                tools_json: Some(tools),
                ..Default::default()
            }],
            ..Default::default()
        };

        // 正面：v1 已知集（8 内置 + 扩展工具）逐个都能通过——拒绝不许拒得比该拒的多
        for name in crate::agent::client::known_tool_names() {
            let inputs = cfg(serde_json::json!([name]));
            validate_startup(&inputs).unwrap_or_else(|e| panic!("已知工具 {name} 不该被拒：{e}"));
        }
        // 未声明（None）与空数组也通过
        assert!(validate_startup(&StartupInputs::default()).is_ok());
        validate_startup(&cfg(serde_json::json!([]))).unwrap();

        // 未知名字 → 拒绝，报文含名字与已知集合
        let err = validate_startup(&cfg(serde_json::json!(["read_file", "web_search"])))
            .unwrap_err()
            .to_string();
        assert!(err.contains("web_search"), "{err}");
        assert!(err.contains("develop"), "{err}");
        assert!(err.contains("v1 已知工具集"), "{err}");
        assert!(err.contains("spawn_sub_agent"), "已知集合含扩展工具：{err}");

        // 形态非法同样拒绝（非数组 / 非字符串项）——旧行为会把它们静默忽略
        let err = validate_startup(&cfg(serde_json::json!({"read_file": true})))
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是数组"), "{err}");
        let err = validate_startup(&cfg(serde_json::json!(["read_file", 42])))
            .unwrap_err()
            .to_string();
        assert!(err.contains("非字符串"), "{err}");
    }
}
