//! 项目 / provider / 阶段配置 / 项目分析（决策 22 / 29 / 46 / 78 / 101 / 103 / 111 / 112 / 130）。

use sqlx::FromRow;

use super::{parse_ts, ts, Store};
use crate::config::{validate_startup, Settings, StartupInputs, StartupReport};
use crate::types::{Project, Provider, StageConfig};
use crate::{Error, Result};

#[derive(Debug, FromRow)]
struct ProjectRow {
    id: String,
    name: String,
    local_path: String,
    default_branch: String,
    language: Option<String>,
    test_framework: Option<String>,
    lint_command: Option<String>,
    agents_md_path: Option<String>,
    created_at: String,
}

impl ProjectRow {
    fn into_project(self) -> Result<Project> {
        Ok(Project {
            id: self.id,
            name: self.name,
            local_path: self.local_path,
            default_branch: self.default_branch,
            language: self.language,
            test_framework: self.test_framework,
            lint_command: self.lint_command,
            agents_md_path: self.agents_md_path,
            created_at: parse_ts(&self.created_at)?,
        })
    }
}

const PROJECT_COLUMNS: &str = "id, name, local_path, default_branch, language, test_framework, \
     lint_command, agents_md_path, created_at";

#[derive(Debug, FromRow)]
struct ProviderRow {
    id: String,
    vendor: String,
    model: String,
    context_window: i64,
    base_url: Option<String>,
    api_key: Option<String>,
    enabled: i64,
    created_at: String,
    updated_at: String,
}

impl ProviderRow {
    fn into_provider(self) -> Result<Provider> {
        Ok(Provider {
            id: self.id,
            vendor: self.vendor,
            model: self.model,
            context_window: self.context_window as u32,
            base_url: self.base_url,
            api_key: self.api_key,
            enabled: self.enabled != 0,
            created_at: parse_ts(&self.created_at)?,
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

const PROVIDER_COLUMNS: &str =
    "id, vendor, model, context_window, base_url, api_key, enabled, created_at, updated_at";

#[derive(Debug, FromRow)]
struct StageConfigRow {
    stage: String,
    provider_id: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    persona_path: Option<String>,
    persona_append: Option<String>,
    tools_json: Option<String>,
    skills_json: Option<String>,
    idle_timeout_sec: Option<i64>,
    max_duration_sec: Option<i64>,
    node_overrides_json: Option<String>,
    updated_at: String,
}

impl StageConfigRow {
    fn into_config(self) -> Result<StageConfig> {
        Ok(StageConfig {
            stage: self.stage,
            provider_id: self.provider_id,
            temperature: self.temperature,
            max_tokens: self.max_tokens.map(|v| v as u32),
            persona_path: self.persona_path,
            persona_append: self.persona_append,
            // 配置在启动时消费（fail fast，决策 103 / 47）：损坏即报错
            tools_json: self
                .tools_json
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            skills_json: self
                .skills_json
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            idle_timeout_sec: self.idle_timeout_sec.map(|v| v as u64),
            max_duration_sec: self.max_duration_sec.map(|v| v as u64),
            node_overrides_json: self
                .node_overrides_json
                .and_then(|s| serde_json::from_str(&s).ok()),
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

impl Store {
    // ─────────────────────────────── 项目 ───────────────────────────────

    pub async fn create_project(&self, project: &Project) -> Result<()> {
        sqlx::query(
            "INSERT INTO kanban_projects
             (id, name, local_path, default_branch, language, test_framework, lint_command,
              agents_md_path, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&project.id)
        .bind(&project.name)
        .bind(&project.local_path)
        .bind(&project.default_branch)
        .bind(&project.language)
        .bind(&project.test_framework)
        .bind(&project.lint_command)
        .bind(&project.agents_md_path)
        .bind(ts(project.created_at))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Option<Project>> {
        let sql = format!("SELECT {PROJECT_COLUMNS} FROM kanban_projects WHERE id = ?");
        let row: Option<ProjectRow> = sqlx::query_as(&sql)
            .bind(project_id)
            .fetch_optional(self.pool())
            .await?;
        row.map(ProjectRow::into_project).transpose()
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let sql = format!("SELECT {PROJECT_COLUMNS} FROM kanban_projects ORDER BY created_at");
        let rows: Vec<ProjectRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(ProjectRow::into_project).collect()
    }

    /// `PATCH /projects/{id}`（决策 101）。
    #[allow(clippy::too_many_arguments)]
    pub async fn update_project(
        &self,
        project_id: &str,
        name: Option<&str>,
        default_branch: Option<&str>,
        test_framework: Option<&str>,
        lint_command: Option<&str>,
    ) -> Result<()> {
        if let Some(v) = name {
            sqlx::query("UPDATE kanban_projects SET name = ? WHERE id = ?")
                .bind(v)
                .bind(project_id)
                .execute(self.pool())
                .await?;
        }
        if let Some(v) = default_branch {
            sqlx::query("UPDATE kanban_projects SET default_branch = ? WHERE id = ?")
                .bind(v)
                .bind(project_id)
                .execute(self.pool())
                .await?;
        }
        if let Some(v) = test_framework {
            sqlx::query("UPDATE kanban_projects SET test_framework = ? WHERE id = ?")
                .bind(v)
                .bind(project_id)
                .execute(self.pool())
                .await?;
        }
        if let Some(v) = lint_command {
            sqlx::query("UPDATE kanban_projects SET lint_command = ? WHERE id = ?")
                .bind(v)
                .bind(project_id)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    /// 有活跃任务时拒绝删除（决策 101）。
    pub async fn project_has_active_tasks(&self, project_id: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_tasks
             WHERE project_id = ? AND archived_at IS NULL
               AND status IN ('queued','waiting','running','pending')",
        )
        .bind(project_id)
        .fetch_one(self.pool())
        .await?;
        Ok(count > 0)
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<()> {
        if self.project_has_active_tasks(project_id).await? {
            return Err(Error::Conflict(
                "项目仍有活跃任务，拒绝删除（决策 101）".into(),
            ));
        }
        sqlx::query("DELETE FROM kanban_projects WHERE id = ?")
            .bind(project_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // ── 项目分析（决策 130 ⑦：202 + 轮询）──

    pub async fn create_analysis(&self, project_id: &str) -> Result<String> {
        let id = ulid::Ulid::new().to_string();
        let now = self.now();
        sqlx::query(
            "INSERT INTO kanban_project_analyses
             (analysis_id, project_id, status, result_json, error, created_at, updated_at)
             VALUES (?, ?, 'running', NULL, NULL, ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(ts(now))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        Ok(id)
    }

    pub async fn finish_analysis(
        &self,
        analysis_id: &str,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_project_analyses
             SET status = ?, result_json = ?, error = ?, updated_at = ?
             WHERE analysis_id = ?",
        )
        .bind(if error.is_some() { "failed" } else { "done" })
        .bind(result.map(|r| r.to_string()))
        .bind(error)
        .bind(ts(self.now()))
        .bind(analysis_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 最近一次分析的状态与结果。
    pub async fn latest_analysis(
        &self,
        project_id: &str,
    ) -> Result<Option<(String, String, Option<serde_json::Value>, Option<String>)>> {
        #[derive(FromRow)]
        struct Row {
            analysis_id: String,
            status: String,
            result_json: Option<String>,
            error: Option<String>,
        }
        let row: Option<Row> = sqlx::query_as(
            "SELECT analysis_id, status, result_json, error FROM kanban_project_analyses
             WHERE project_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| {
            (
                r.analysis_id,
                r.status,
                r.result_json
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                r.error,
            )
        }))
    }

    // ─────────────────────────────── provider ───────────────────────────────

    /// 明文存储（决策 112）；读接口由 [`Provider::masked_api_key`] 回显 `***`。
    pub async fn upsert_provider(&self, provider: &Provider) -> Result<()> {
        let now = self.now();
        sqlx::query(
            "INSERT INTO providers
             (id, vendor, model, context_window, base_url, api_key, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 vendor = excluded.vendor, model = excluded.model,
                 context_window = excluded.context_window, base_url = excluded.base_url,
                 api_key = excluded.api_key, enabled = excluded.enabled,
                 updated_at = excluded.updated_at",
        )
        .bind(&provider.id)
        .bind(&provider.vendor)
        .bind(&provider.model)
        .bind(provider.context_window as i64)
        .bind(&provider.base_url)
        .bind(&provider.api_key)
        .bind(if provider.enabled { 1 } else { 0 })
        .bind(ts(if provider.created_at.timestamp() == 0 {
            now
        } else {
            provider.created_at
        }))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 原始 provider 行（含明文 api_key）——**仅启动校验与运行时适配使用**。
    pub async fn load_providers(&self) -> Result<Vec<Provider>> {
        let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers ORDER BY created_at");
        let rows: Vec<ProviderRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(ProviderRow::into_provider).collect()
    }

    /// 读接口回显（决策 112）：不返回原值。
    pub async fn list_providers_masked(&self) -> Result<Vec<Provider>> {
        Ok(self
            .load_providers()
            .await?
            .into_iter()
            .map(|mut p| {
                p.api_key = p.masked_api_key();
                p
            })
            .collect())
    }

    pub async fn get_provider(&self, provider_id: &str) -> Result<Option<Provider>> {
        let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE id = ?");
        let row: Option<ProviderRow> = sqlx::query_as(&sql)
            .bind(provider_id)
            .fetch_optional(self.pool())
            .await?;
        row.map(ProviderRow::into_provider).transpose()
    }

    pub async fn set_provider_enabled(&self, provider_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE providers SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(if enabled { 1 } else { 0 })
            .bind(ts(self.now()))
            .bind(provider_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn delete_provider(&self, provider_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(provider_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // ─────────────────────────────── 阶段配置 ───────────────────────────────

    pub async fn upsert_stage_config(&self, cfg: &StageConfig) -> Result<()> {
        sqlx::query(
            "INSERT INTO stage_configs
             (stage, provider_id, temperature, max_tokens, persona_path, persona_append,
              tools_json, skills_json, idle_timeout_sec, max_duration_sec, node_overrides_json,
              updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(stage) DO UPDATE SET
                 provider_id = excluded.provider_id, temperature = excluded.temperature,
                 max_tokens = excluded.max_tokens, persona_path = excluded.persona_path,
                 persona_append = excluded.persona_append, tools_json = excluded.tools_json,
                 skills_json = excluded.skills_json, idle_timeout_sec = excluded.idle_timeout_sec,
                 max_duration_sec = excluded.max_duration_sec,
                 node_overrides_json = excluded.node_overrides_json,
                 updated_at = excluded.updated_at",
        )
        .bind(&cfg.stage)
        .bind(&cfg.provider_id)
        .bind(cfg.temperature)
        .bind(cfg.max_tokens.map(|v| v as i64))
        .bind(&cfg.persona_path)
        .bind(&cfg.persona_append)
        .bind(cfg.tools_json.as_ref().map(|v| v.to_string()))
        .bind(cfg.skills_json.as_ref().map(|v| v.to_string()))
        .bind(cfg.idle_timeout_sec.map(|v| v as i64))
        .bind(cfg.max_duration_sec.map(|v| v as i64))
        .bind(cfg.node_overrides_json.as_ref().map(|v| v.to_string()))
        .bind(ts(self.now()))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn list_stage_configs(&self) -> Result<Vec<StageConfig>> {
        let rows: Vec<StageConfigRow> = sqlx::query_as(
            "SELECT stage, provider_id, temperature, max_tokens, persona_path, persona_append,
                    tools_json, skills_json, idle_timeout_sec, max_duration_sec,
                    node_overrides_json, updated_at
             FROM stage_configs ORDER BY stage",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(StageConfigRow::into_config).collect()
    }

    pub async fn get_stage_config(&self, stage: &str) -> Result<Option<StageConfig>> {
        Ok(self
            .list_stage_configs()
            .await?
            .into_iter()
            .find(|c| c.stage == stage))
    }

    /// 删除阶段覆盖（回到系统默认，决策 22）。返回是否确实删了一行。
    pub async fn delete_stage_config(&self, stage: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM stage_configs WHERE stage = ?")
            .bind(stage)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// 启动校验：加载 DB 配置 → fail fast 或降级（决策 47 / 103 / 134）。
    ///
    /// 不受支持的 vendor **降级 `enabled = 0`**（不崩溃）；被阶段引用的则 fail fast。
    pub async fn validate_startup(&self, settings: &Settings) -> Result<StartupReport> {
        let providers = self.load_providers().await?;
        let stage_configs = self.list_stage_configs().await?;
        let report = validate_startup(&StartupInputs {
            settings: settings.clone(),
            providers: providers.clone(),
            stage_configs,
            // 决策 47 / 170 / 172：对"真实可用"的 skill 集合 fail fast
            // ——PATH 可执行文件 ∪ 内嵌知识型技能 ∪ 技能根下的用户覆盖
            available_skills: crate::config::discover_available_skills(&self.home().skills_dir()),
            // §10.6.4：persona_path 相对 home 根解析，启动时一并校验存在且非空
            home_root: Some(self.home().root().to_path_buf()),
            // 决策 170 / 172：知识型技能的正文必须存在且非空，frontmatter name 须与目录名一致
            skills_root: Some(self.home().skills_dir()),
        })?;
        for id in &report.demoted_providers {
            self.set_provider_enabled(id, false).await?;
        }
        Ok(report)
    }
}

/// provider 解析优先级（决策 129）：
/// `node_overrides > task.model_override > 阶段 provider > 全局默认`。
pub fn resolve_provider_id(
    node_override: Option<&str>,
    task_model_override: Option<&str>,
    stage: Option<&StageConfig>,
    global_default: Option<&str>,
) -> Option<String> {
    node_override
        .map(str::to_string)
        .or_else(|| task_model_override.map(str::to_string))
        .or_else(|| stage.and_then(|c| c.provider_id.clone()))
        .or_else(|| global_default.map(str::to_string))
}

/// 节点级 provider 覆盖（`node_overrides_json.provider_id`，决策 129）。
pub fn node_provider_override(stage: Option<&StageConfig>, node: &str) -> Option<String> {
    stage
        .and_then(|c| c.node_overrides_json.as_ref())
        .and_then(|v| v.get(node))
        .and_then(|n| n.get("provider_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_resolution_precedence() {
        let stage = StageConfig {
            stage: "develop".into(),
            provider_id: Some("stage-p".into()),
            ..Default::default()
        };
        // 节点级最高
        assert_eq!(
            resolve_provider_id(
                Some("node-p"),
                Some("task-p"),
                Some(&stage),
                Some("global-p")
            )
            .as_deref(),
            Some("node-p")
        );
        // 任务级次之（决策 105 / 129）
        assert_eq!(
            resolve_provider_id(None, Some("task-p"), Some(&stage), Some("global-p")).as_deref(),
            Some("task-p")
        );
        // 再退阶段配置
        assert_eq!(
            resolve_provider_id(None, None, Some(&stage), Some("global-p")).as_deref(),
            Some("stage-p")
        );
        // 兜底全局默认
        assert_eq!(
            resolve_provider_id(None, None, None, Some("global-p")).as_deref(),
            Some("global-p")
        );
        assert_eq!(resolve_provider_id(None, None, None, None), None);
    }

    #[test]
    fn node_override_reads_provider_id() {
        let stage = StageConfig {
            stage: "merge".into(),
            node_overrides_json: Some(serde_json::json!({
                "execute": {"provider_id": "long-ctx"},
                "validate_output": {"idle_timeout_sec": 60}
            })),
            ..Default::default()
        };
        assert_eq!(
            node_provider_override(Some(&stage), "execute").as_deref(),
            Some("long-ctx")
        );
        assert_eq!(
            node_provider_override(Some(&stage), "validate_output"),
            None
        );
        assert_eq!(node_provider_override(None, "execute"), None);
    }
}
