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
        );
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

/// 发现"可用 skill"（决策 47，**已由决策 170 / 172 修订**——两类来源）。
///
/// 保留决策 47 的工具语义（`rtk` / `codegraph` 等 CLI，以 PATH 可执行文件名为准），
/// 并加入**用户 markdown** 知识型技能（技能根下的 `{name}/SKILL.md`；内嵌技能已随
/// 决策 172① 退场）。技能正文的注入见 [`crate::agent::skills::resolve`] 与
/// [`crate::agent::prompts::build_system_prompt`]。
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

/// 完整配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub pipeline: PipelineOverrides,
    pub logging: LoggingConfig,
    pub prompts: PromptsConfig,
    pub skills: SkillsConfig,
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
        // 技能校验（决策 47 / 170 / 172④）：阶段级与节点级声明过同一套检查——
        // ① 声明形态合法（未知 mode / 未信任 + full → 拒绝，见 parse_skill_decls）；
        // ② 名字必须存在于可用技能集；③ 知识型技能的正文必须存在且非空。
        for (where_, decl) in declared_skills(cfg)? {
            let skill = &decl.name;
            if !inputs.available_skills.iter().any(|s| s == skill) {
                return Err(Error::Config(format!(
                    "{where_} 引用了不存在的 skill：{skill}"
                )));
            }
            // 提供技能根时校验正文（工具型技能无正文，`resolve` 返回 None 不报错）。
            // 名字态不读正文（票 05）——正文由 `Skill` 工具按需拉取，此处不校验其存在性。
            if let Some(root) = &inputs.skills_root {
                if decl.mode == crate::agent::skills::SkillMode::Full {
                    crate::agent::skills::resolve(root, std::slice::from_ref(&decl)).map_err(
                        |e| Error::Config(format!("{where_} 的 skill {skill} 不可用：{e}")),
                    )?;
                }
            }
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
    use crate::types::Stage;

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
}
