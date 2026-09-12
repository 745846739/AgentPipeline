//! 全局配置与阶段级覆盖（docs/overview.md §3、docs/agents.md §10.6）。
//!
//! config.toml 只保留 `[server]` / `[pipeline]` / `[logging]` / `[prompts]`（决策 56）；
//! provider 与阶段配置存 DB（决策 22 / 111）。

use std::path::Path;

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
#[serde(default)]
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
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    /// 绑定地址（§10.6.5：`[server] host`）。只允许 IP 字面量。
    pub host: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            port: 8787,
            host: "127.0.0.1".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub json_file: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".to_string(),
            json_file: true,
        }
    }
}

/// 发现"可用 skill"（决策 47）：skill 的语义是"用户机器上装了对应的外部工具"
/// （rtk / codegraph 等 CLI），以 PATH 中可执行文件的文件名为准。
pub fn discover_available_skills() -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let is_executable = entry.metadata().map(|m| m.is_file()).unwrap_or(false) && {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        entry
                            .metadata()
                            .map(|m| m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                };
                if is_executable {
                    if let Some(name) = entry.file_name().to_str() {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PromptsConfig {
    /// 覆盖 prompt 的目录；缺省时用 `{home}/prompts`。
    pub dir: Option<String>,
}

/// 完整配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub pipeline: PipelineOverrides,
    pub logging: LoggingConfig,
    pub prompts: PromptsConfig,
}

impl Config {
    /// 从 TOML 文本解析。
    pub fn from_toml(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::Config(format!("config.toml 解析失败：{e}")))
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

/// 启动校验的输入（provider / 阶段配置来自 DB）。
#[derive(Debug, Clone, Default)]
pub struct StartupInputs {
    pub settings: Settings,
    pub providers: Vec<Provider>,
    pub stage_configs: Vec<StageConfig>,
    /// 本机可用的 skill 名（配置里引用不存在的 skill → fail fast）。
    pub available_skills: Vec<String>,
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
/// - 引用的 skill 不存在 → 拒绝启动；
/// - provider 表里 vendor 不受支持但**未被引用** → 降级 `enabled = 0`，只报告不报错。
pub fn validate_startup(inputs: &StartupInputs) -> Result<StartupReport> {
    let mut report = StartupReport::default();
    let mut usable: Vec<&Provider> = Vec::new();

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
        if let Some(skills) = cfg.skills_json.as_ref().and_then(|v| v.as_array()) {
            for skill in skills.iter().filter_map(|v| v.as_str()) {
                if !inputs.available_skills.iter().any(|s| s == skill) {
                    return Err(Error::Config(format!(
                        "阶段 {} 引用了不存在的 skill：{skill}",
                        cfg.stage
                    )));
                }
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
}
