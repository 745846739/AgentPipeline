//! CLI 引擎的 HTTP 通道（决策 427，票 talk-cli-engine 05）：提议创建 + DB 读面。
//!
//! stdio MCP 代理是独立子进程，它够不到 API 值班长的进程内执行点——这两根端点是它
//! 唯一的翻译目标。**执行面零新增**：提议只在这里落一行，执行仍走既有
//! `POST /foreman/proposals/{id}/execute`（人按键 → 主进程进程内执行器）。
//!
//! 两道闸（决策 427⑤）：配对令牌是机器级第一道（全局 `pairing_guard` 已护住本族）；
//! 第二道在这里——session 必须存在且 `engine=cli`，API 班次不收一个来路不明的本地
//! 进程塞进来的提议与读数。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::file_policy::foreman_file_policy;
use agentpipeline_core::agent::tools::{
    CommandFinish, CommandRecorder, CommandStart, ToolCallContext, ToolExecutor,
};
use agentpipeline_core::pipeline::foreman::{
    validate_args_against_spec, CLI_PROPOSAL_TOOLS, CLI_READ_TOOLS,
};
use agentpipeline_core::process::RealProcessKiller;
use agentpipeline_core::storage::foreman::FOREMAN_ENGINE_CLI;
use agentpipeline_core::types::{CommandSource, Node, Stage};

use super::foreman::proposal_wire;
use crate::state::AppState;
use crate::state::{map_core_error, ApiError, ApiResult};

/// `POST /foreman/proposals`：CLI 通道落一条提议（决策 427①）。
///
/// 载荷 `{session_id, tool, args, summary}`。校验面收在这一处：工具在暴露子集
/// （[`CLI_PROPOSAL_TOOLS`]）、参数过 catalog schema（浅校验）、summary 非空、
/// session 存在且 engine=cli。TTL 与既有的提议同律——TTL 是存储层的规则，
/// 不由调用方给。
pub async fn create_proposal(
    State(state): State<AppState>,
    Json(body): Json<CreateProposalBody>,
) -> ApiResult<Json<Value>> {
    let session = cli_session(&state, &body.session_id).await?;
    if body.summary.trim().is_empty() {
        return Err(ApiError::bad_request(
            "summary 不能为空——那是人按键之前唯一读的那一句",
        ));
    }
    let args = body.args;
    if !args.is_object() {
        return Err(ApiError::bad_request(format!(
            "工具 {} 的参数必须是 JSON 对象",
            body.tool
        )));
    }
    if !CLI_PROPOSAL_TOOLS.contains(&body.tool.as_str()) {
        return Err(ApiError::bad_request(format!(
            "工具 {} 不在 CLI 通道的暴露名单里（可选：{}）",
            body.tool,
            CLI_PROPOSAL_TOOLS.join(" / ")
        )));
    }
    validate_args_against_spec(&body.tool, &args)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let proposal = agentpipeline_core::pipeline::proposals::create_cli_proposal(
        &state.store,
        &(state.sse.clone() as Arc<dyn agentpipeline_core::sse::SseSink>),
        &session.id,
        &body.tool,
        &args,
        body.summary.trim(),
    )
    .await
    .map_err(map_core_error)?;
    Ok(Json(proposal_wire(&proposal)))
}

/// `GET /foreman/read/{tool}`：CLI 读面的 DB 读数（决策 427③）。
///
/// 通用端点、一个校验面：`tool` 在 [`CLI_READ_TOOLS`]、`session` 存在且 engine=cli、
/// `args` 查询参数是一段 JSON 对象并过同一个 schema 浅校验。执行走**进程内读工具的
/// 同一个入口**（`ToolExecutor::execute`，allowed 只放这九个），每次读落一条命令台账
/// （会话归属）——HTTP 读面是新开的嘴，审计纪律与进程内读数同律。
pub async fn read_tool(
    State(state): State<AppState>,
    Path(tool): Path<String>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Json<Value>> {
    let ReadQuery { session, args } = query;
    let session_row = cli_session(&state, &session).await?;
    if !CLI_READ_TOOLS.contains(&tool.as_str()) {
        return Err(ApiError::bad_request(format!(
            "工具 {tool} 不在 CLI 读面上（可选：{}）",
            CLI_READ_TOOLS.join(" / ")
        )));
    }
    let args = match args {
        Some(raw) => {
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|e| ApiError::bad_request(format!("args 不是合法 JSON：{e}")))?;
            if !parsed.is_object() {
                return Err(ApiError::bad_request("args 必须是 JSON 对象"));
            }
            parsed
        }
        None => Value::Object(serde_json::Map::new()),
    };
    validate_args_against_spec(&tool, &args).map_err(|e| ApiError::bad_request(e.to_string()))?;

    // 读执行器：allowed 只放九个 DB 读数——文件 / 进程 / 网络类的嘴不在这一面上
    // （那些 CLI 原生工具承担，决策 427③）。不注入记录器与提议 sink：读工具用不到，
    // 命令台账由下面的显式两步记（读数不是命令，但这条 HTTP 通道的每次动用都要留痕）。
    let executor = ToolExecutor::new(
        state.home.clone(),
        foreman_file_policy(state.home.root(), state.settings.file_access_unrestricted),
        state.settings.clone(),
        Arc::new(RealProcessKiller),
    )
    .with_ledger(state.store.clone())
    .with_allowed_tools(CLI_READ_TOOLS.to_vec());
    // 上下文里的占位值是**有意的**：读面是班次级的嘴，不挂任务也不进流水线——
    // task_id 空、stage/node 只是台账要个归属（Init/Execute），路径只对文件族工具有
    // 意义而这九个读数一个都不碰文件；allowed 收死在九个读工具上，占位值进不了执行。
    let ctx = ToolCallContext {
        task_id: String::new(),
        session_id: Some(session_row.id.clone()),
        stage: Stage::Init,
        node: Node::Execute,
        worktree_path: std::path::PathBuf::from(state.home.root()),
        task_dir: std::path::PathBuf::from(state.home.root()),
        run_id: None,
        command_source: CommandSource::Agent,
        default_cwd: None,
    };
    let started = std::time::Instant::now();
    let call = ToolCall {
        id: ulid::Ulid::new().to_string(),
        name: tool.clone(),
        arguments: args.to_string(),
    };
    // 台账**先起账再执行**（决策 427③「每次读落命令台账」）：动用即留痕，失败也是一次
    // 动用——起账放在执行前，失败才不会在台账上「没发生过」。命令串记的是这次工具调用的
    // 原文（截到 200 字符）。没有进程跑起来，退出码记的是「这次读成没成」：成功 0、
    // 失败 1（错误文本进 stderr 预览），stdout 预览不重复存正文。
    let command_text = format!("{tool} {args}");
    let command_id = state
        .store
        .record_start(CommandStart {
            task_id: None,
            session_id: Some(session_row.id.clone()),
            run_id: None,
            stage: Stage::Init,
            node: Node::Execute,
            source: CommandSource::Agent,
            command: command_text.chars().take(200).collect(),
            cwd: state.home.root().display().to_string(),
            original_command: None,
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let outcome = executor.execute(&call, &ctx).await;
    let failed = outcome.is_err();
    let error_text = outcome.as_ref().err().map(|e| e.to_string());
    let _ = state
        .store
        .record_finish(
            command_id,
            CommandFinish {
                exit_code: Some(i32::from(failed)),
                stdout_path: None,
                stdout_preview: None,
                stderr_preview: error_text.map(|e| e.chars().take(200).collect()),
                duration_ms: started.elapsed().as_millis() as u64,
            },
        )
        .await;
    let outcome = outcome.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "tool": tool,
        "content": outcome.content,
    })))
}

/// 第二道闸：session 存在且 engine=cli（决策 427⑤）。
///
/// 404 与 400 分开说：不存在是「没有这个班次」，引擎不对是「这个班次不收 CLI 通道的
/// 东西」——排障时这两句指向完全不同的方向。
async fn cli_session(
    state: &AppState,
    session_id: &str,
) -> ApiResult<agentpipeline_core::storage::foreman::ForemanSession> {
    let Some(session) = state
        .store
        .get_foreman_session(session_id)
        .await
        .map_err(map_core_error)?
    else {
        return Err(ApiError::not_found(format!("班次不存在：{session_id}")));
    };
    if session.engine != FOREMAN_ENGINE_CLI {
        return Err(ApiError::bad_request(
            "该班次不是 CLI 引擎——CLI 通道（提议创建 / DB 读面）只服务 engine=cli 的班次",
        ));
    }
    Ok(session)
}

#[derive(Deserialize)]
pub struct CreateProposalBody {
    pub session_id: String,
    pub tool: String,
    pub args: Value,
    pub summary: String,
}

#[derive(Deserialize)]
pub struct ReadQuery {
    pub session: String,
    /// 参数原文（一段 JSON 对象）。放单个查询参数而不是逐键展开：类型信息（integer /
    /// boolean）在 JSON 里原本就有，逐键的 query 形态会把所有值压成字符串，schema
    /// 校验就得在端点上再猜一遍类型——JSON 进 JSON 出，校验面只过一遍。
    pub args: Option<String>,
}
