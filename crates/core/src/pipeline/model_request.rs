//! 模型请求组装（决策 249 · 片①，票 01）：prompt 组装 + 上下文预算收成一片。
//!
//! 五片拆分的**叶子**：调用方只有模型调用编排（票 03；当前由 `executor::agent_attempt_inner`
//! 经本 interface 调用）。职责边界——
//!
//! - **组装**（[`RequestPlan::assemble`]，每 attempt 恰一次）：system/user 两段逐字 prompt
//!   （决策 211②：原文是权威）、工具定义、上下文容量一次拼齐；`system/user/hash` 该 attempt
//!   内冻结（prompt cache，§12.13.5）——重试轮**重新 assemble**（段与台账状态变了），
//!   同一行调用、零段参数。组装期唯一能判的超限（静态两段已超硬限，压缩只动 messages、
//!   救不回来）以 [`Prepared::Overflow`] 返回：**臂上带 plan**——超限时先落原文再收口是
//!   形状保证，不是纪律（决策 180 退出路径条件）。
//! - **每轮预算门**（[`RequestPlan::check_budget`]，同步）：L3 规则化按轮就地压缩；
//!   越界返回 [`BudgetCheck::Overflow`]——**值不是 Pending**：翻译（先落快照与会话行、
//!   再 `NodeOutput::Pending(context_overflow)`）留在编排侧（决策 245「门吃落点不吃原因」，
//!   决策 154 的构造在门外）。
//!
//! 签名之外编排侧必须知道的不变量：
//!
//! 1. `assemble` 每 attempt 恰一次；hash 对**全文态技能正文**敏感、对名字态钝感（决策 170）。
//! 2. `carried_len` = 续接锚点（决策 180：压缩锚不认载入历史），调用方在进轮循环前取。
//! 3. 任一门 `Overflow` → 先落快照与会话行再翻译成 Pending，**永不继续循环**（决策 154）。
//! 4. `capacity = None` → 跳过预算门恒 Ok（决策 110：无 provider 不臆造窗口）。
//! 5. **system 侧不开扩展轴**：golden 序与 prompt cache 是决策，封顶是有意的。
//! 6. `plan_custom` **不在本 interface**——伪阶段/repair 真成为第二条调用方时单开票进
//!    （两条 adapter 才是真 seam）。
//! 7. 可删条款已执行：实现期窄测试全走写文件/建行取段，`assemble_with` 未落地——
//!    注入轴等真有第二调用方再开。
//!
//! 依赖四分类（DEEPENING）：组装/hash/预算谓词是 in-process 纯计算；fs 经 `Home` 临时目录、
//! 库经临时 SQLite 是 local-substitutable。**无 git、无 LLM、无 `Executor`，不开 port**——
//! internal seam 足够，interface 就是测面。

use std::path::Path;

use crate::agent::bounded_read::{self, Offloaded};
use crate::agent::client::{LlmRequest, Message, RunContext, ToolDef};
use crate::agent::context::{
    compact_messages_from, count_tokens, estimate_context_capacity, over_hard_limit,
    should_compact_with_floor, transcript_chars, ContextCapacity,
};
use crate::agent::prompts::{
    build_system_prompt, build_user_prompt, load_agents_context, prompt_template_hash,
    render_template, resolve_persona, PromptSegments, TemplateVars,
};
use crate::agent::templates::{system_template, user_template};
use crate::agent::{effective_skills, effective_tools, submit_metadata_tool, SKILL_TOOL};
use crate::config::Settings;
use crate::home::Home;
use crate::storage::observability::PromptSnapshot;
use crate::storage::{AttentionKind, Store};
use crate::types::{Node, NodeCursor, Project, Stage, StageConfig, Task};
use crate::{Error, Result};

use super::events::{
    OUTPUT_CODE_CHANGES, OUTPUT_DESIGN_DOC, OUTPUT_DEV_DOC, OUTPUT_REVIEW_REPORT,
    OUTPUT_TEST_REPORT, OUTPUT_TEST_SCENARIOS,
};
use super::merge::test_command_for;
use super::model_invoke::AgentNodeKind;

/// 一次 attempt 的身份事实 + 仅有的两个环境依赖（store / settings）。
///
/// 结构进 interface：D 桶窄测试经它直连临时 SQLite，不建 `Executor`。
pub struct AttemptCtx<'a> {
    pub store: &'a Store,
    pub settings: &'a Settings,
    pub task: &'a Task,
    pub project: &'a Project,
    /// stage + node：段门（首轮为空不渲染）与记账都看它。
    pub cursor: &'a NodeCursor,
    pub stage_cfg: Option<&'a StageConfig>,
    pub attempt: u32,
    /// 节点种类：`submit_metadata` 的 schema 工具按它分发（与 `tool_defs` 同源）。
    pub kind: AgentNodeKind,
    /// 决策 279：补充输入已作为 user turn 追加进续接转录（validate_input 的续接场景）
    /// ——「用户补充输入」segment 停止渲染，否则同一段话出现两遍、首条消息还变了
    /// （prompt cache 整段打穿的实测根源）。execute 等其余场景恒 false，segment 照旧。
    pub user_input_as_turn: bool,
    /// 决策 387：review 打回反馈已作为 user turn 追加进续接转录（develop.execute 的
    /// 打回续接场景）——「评审必须修改项」segment 停止渲染，否则同一段话出现两遍、
    /// 首条消息还变了。只在真续接了转录时为 true；无转录的重入段照旧（降级通道）。
    pub review_rework_as_turn: bool,
    /// 票 04：超时梯子第 3 档（空白重跑）的起跑简报——**已组装好的文本**，本模块只负责
    /// 渲染进首条 user prompt。文本由 `pipeline::continuation_brief` 拼（那一步要读盘 /
    /// 读 git / 读库），本模块因此仍守着「无 git」的边界（见模块 doc 的依赖四分类）。
    pub continuation_brief: Option<String>,
    /// 启动时探到的完全磁盘访问授权（决策 306）。
    ///
    /// **值注入，不是缝**：进程级快照由启动探测写入（`agent::disk_access::record`），
    /// 执行体在这里把它填进来；测试直接置值。照第五条接缝
    /// （`AGENTPIPELINE_MARKET_GIT_BASE` 那条「远端地址替换点」）的**非类型缝**先例——
    /// 明确不新造 `DiskAccessProbe` trait（决策 250 刚删掉一条只有一处 `impl` 的假 seam）。
    ///
    /// 为什么必须在**读之前**用快照判：`access()` / `stat()` 对受保护路径同样会阻塞，
    /// 在读的那一刻探等于把「检查授权」变成第二个挂起现场。
    pub disk_access: crate::agent::disk_access::DiskAccessState,
}

/// 组装期判出的超限事实。**不是 `PendingReason`**——落点由构造者定（决策 245），
/// 这里只带门的读数，供翻译侧记录观测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowFacts {
    /// 静态两段（system + user）的 token 估算。
    pub estimate: usize,
    pub hard_limit: usize,
}

/// 组装结果。常态一支直接给 plan；超限一支**也带 plan**——先落原文再收口是形状保证
/// （决策 180 退出路径条件：会话行要带 prompt 快照，plan 没了快照就没了）。
#[derive(Debug)]
pub enum Prepared {
    Ready(RequestPlan),
    Overflow {
        plan: RequestPlan,
        facts: OverflowFacts,
    },
}

/// 每轮预算门的返回：值不是 Pending（越界翻译照决策 245 留在编排侧）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetCheck {
    /// 预算内。`compacted` = L3 本轮压掉的消息条数（`None` = 没触发压缩）。
    Ok { compacted: Option<usize> },
    /// 压缩后仍超硬限。`estimate` 是**压缩后**的读数——压缩救不回来才叫越界。
    Overflow { estimate: usize, hard_limit: usize },
}

/// 一次 attempt 拼齐的请求计划：自持 String、无生命周期，编排侧跨轮自由持有。
///
/// 除决策 249 定稿的七项外带了 LlmRequest 的身份戳（task/stage/node/attempt）与
/// `keep_recent_rounds`——「以实现为准微调」条款：`check_budget` / `request` 是同步自持的，
/// 不能回头借 `AttemptCtx` / `Settings`。
#[derive(Debug)]
pub struct RequestPlan {
    /// system 逐字（决策 211②：原文是权威；hash 对全文态技能正文敏感，决策 170）。
    pub system: String,
    /// user 逐字（含五追加段渲染结果）。
    pub user: String,
    /// system 全文态的 SHA-256 前 16 位——索引，原文才是权威（决策 137 / 211②）。
    pub hash: String,
    /// 工具定义（deny 档摘除环境层广告，决策 206；每 attempt 拼一次、轮内冻结）。
    pub tools: Vec<ToolDef>,
    /// `None` = 无可用 provider，跳过分档（决策 110：不臆造窗口）。
    pub capacity: Option<ContextCapacity>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// 任务级 provider 覆盖（决策 105）；阶段配置/系统默认由生产适配器解析。
    pub provider_id: Option<String>,
    pub task_id: String,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    pub keep_recent_rounds: usize,
    /// L3 硬底（long-run-budget 票 02，修订票 03）：转录 token 估算超过它就强制压缩，
    /// 与软限判据取「或」。绝对数、不看 provider 窗口登记的脸色。
    /// （字符线 `Settings.conversation_max_chars` 只管会话落库截断，不再参与这里的触发。）
    pub conversation_max_tokens: usize,
}

impl RequestPlan {
    /// 默认入口：常见路径这一行（零段参数——段自己去取；重试轮同形重调）。
    pub async fn assemble(ctx: AttemptCtx<'_>) -> Result<Prepared> {
        // **缺授权时快速失败**（决策 306）：先给一句可操作的话，**不等那 300 秒**。
        // 放在取段之前——这一层判的是环境，不是这一轮的输入。判据取自**值**（`ctx.disk_access`，
        // 启动探测写进快照、执行体填进来的那一份），不在读的那一刻现探（`access()` / `stat()`
        // 对受保护路径同样会阻塞，现探等于把「检查授权」变成第二个挂起现场）。
        //
        // 节点因此**立刻**转 pending（错误沿 `model_invoke` 的 `?` 出去，落进执行体既有的
        // 错误路径），而**不会产生任何超时记账**：这里连一次模型调用都还没发出去。
        // 判据是**两个条件的合取**：快照说缺授权，**且**这一轮的根落在受保护的地方
        // （`~/Documents` / `~/Desktop` / `~/Downloads`）——只有那时缺授权才会让读挂住。
        // 只看前者会把每一台没开授权的机器都变成跑不动（实现期实测踩到：`smoke` /
        // `restart_recovery` 两条真二进制用例当场红）。
        if let Some(err) = ctx
            .disk_access
            .denied_error_for_project(Path::new(&ctx.project.local_path))
        {
            return Err(err);
        }
        let segments = load_segments(&ctx).await?;
        Self::assemble_seeded(ctx, segments).await
    }

    /// 组装主体。独立于此的存在理由：`load_segments` 是异步取数，与纯组装分开后
    /// 前者的读点集中、后者全程可同步推理。
    async fn assemble_seeded(ctx: AttemptCtx<'_>, segments: PromptSegments) -> Result<Prepared> {
        let home = ctx.store.home();
        let cursor = ctx.cursor;
        let worktree = ctx
            .task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&ctx.task.id).display().to_string());
        let task_dir = home.task_dir(&ctx.task.id).display().to_string();

        // 技能：阶段级 ∪ 节点级（决策 170 / 172④），再解析成渲染形态——全文态注入正文、
        // 名字态只列名字（正文交给 `Skill` 工具按需拉取，票 06）、目录态给出「还有哪些
        // 技能可用」（渐进披露）。技能根经 `Home::skills_dir` 取（默认 `{home}/skills`，
        // `[skills] dir` 可覆盖，决策 172）。
        //
        // 两处读都走**有界阻塞读**（决策 302，票 01）：技能根可配置，指到受保护路径时
        // 与 AGENTS.md 是同一个卡死形状。
        let skills_root = home.skills_dir();
        let mut declared = crate::config::stage_skills(ctx.stage_cfg)?;
        declared.extend(crate::config::node_skills(
            ctx.stage_cfg,
            cursor.node.as_str(),
        )?);
        let declared = effective_skills(&declared);
        let declared_names = crate::agent::baseline::effective_skill_names(&declared);
        // 声明的技能排在目录之前（保持 golden 顺序「先看已启用的」）
        let mut skills = crate::agent::skills::resolve_bounded(&skills_root, &declared).await?;
        skills.extend(crate::agent::skills::catalogue_bounded(&skills_root, &declared_names).await);

        // system prompt：[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能][格式规则]
        let persona = resolve_stage_persona(home, ctx.stage_cfg, cursor.stage, cursor.node).await?;
        let system = build_system_prompt(
            &load_agents_context(
                Path::new(&ctx.project.local_path),
                ctx.project.language.as_deref(),
                ctx.project.test_framework.as_deref(),
            )
            .await,
            &persona,
            &workdirs_line(&worktree, &task_dir),
            &skills,
        );
        let vars = template_vars(ctx.store, ctx.task, ctx.project, &worktree, &task_dir).await?;
        // user prompt：§10.3 节点模板 + G12 环境路径块 + 可选追加段（首轮为空不渲染）
        let user = build_user_prompt(
            &format!(
                "{}\n\n## 环境路径\n{}",
                render_template(user_template(cursor.stage, cursor.node), &vars),
                workdirs_line(&worktree, &task_dir)
            ),
            &segments,
        );
        let hash = prompt_template_hash(&system);

        // ── 卡住的读在累积 → 落一条待办（决策 308，票 07）──────────────────────────
        //
        // 计数器是**进程级**的（读点深在调用链里，拿不到 store 句柄），而待办表按**任务**
        // 记账——两者在这里交汇一次：`take_attention_due` 靠一次 swap 保证一次发作只让
        // 一个调用方拿到账，于是只落一条。落点在读做完之后、容量分档之前，超限那条早退
        // 路径也照落（它是同一个组装里的事实）。
        //
        // 落账失败只记日志、不拖累组装：这是一条观测，不是组装的一部分。
        if bounded_read::take_attention_due() {
            let s = bounded_read::stats();
            if let Err(e) = ctx
                .store
                .note_attention(
                    &ctx.task.id,
                    AttentionKind::BlockedRead,
                    ctx.store.now(),
                    Some(&serde_json::json!({
                        "stuck_now": s.stuck_now,
                        "stuck_total": s.stuck_total,
                        "longest_wait_ms": s.longest_wait_ms,
                        "threshold": bounded_read::STUCK_READ_ATTENTION_THRESHOLD,
                        "blocking_pool_cap": bounded_read::BLOCKING_POOL_CAP,
                        "stage": cursor.stage.as_str(),
                        "node": cursor.node.as_str(),
                    })),
                )
                .await
            {
                tracing::warn!(task = %ctx.task.id, error = %e, "卡住的读待办落账失败");
            }
        }

        // 工具与技能同源（阶段声明 + 档位）：deny 档连广告都不给（决策 206）；
        // 未知工具名 fail fast（决策 154 的后续票，报文与写入侧校验同源）。
        let env_mode = crate::types::effective_env_mode(
            ctx.settings.env_mode,
            cursor.stage.as_str(),
            ctx.stage_cfg,
        );
        let declared_tools = effective_declared_tools(
            cursor.stage,
            ctx.stage_cfg.and_then(|c| c.tools_json.as_ref()),
        );
        // 外发开关开 → 广告集追加 `offload_run`（票 runner-offload/06）。**现读**（与 rtk
        // 开关同一姿态，决策 297）：保存即活。关着 = 广告里根本没有它（基线不含它，
        // 见 `client::BUILTIN_TOOLS` 的说明），模型的窗口一个字节不多花。`deny` 档下
        // 不追加——环境层整层都不在，单独留一个能跑远端命令的口子是漏。
        let declared_tools = match ctx.store.offload_switch().await {
            Ok(s) if s.enabled && env_mode != crate::types::EnvMode::Deny => {
                let mut with_offload = declared_tools;
                if !with_offload
                    .iter()
                    .any(|t| t == crate::agent::catalog::OFFLOAD_RUN)
                {
                    with_offload.push(crate::agent::catalog::OFFLOAD_RUN.to_string());
                }
                with_offload
            }
            Ok(_) => declared_tools,
            Err(e) => {
                tracing::warn!(error = %e, "读外发开关失败，按关处理（广告侧）");
                declared_tools
            }
        };
        let tools = tool_defs(
            ctx.kind,
            &declared_tools,
            &skills,
            env_mode,
            cursor.stage,
            cursor.node,
        )?;

        // L0 容量预估（决策 110 / 票 04）：窗口来自解析后的 provider 行
        // （`providers.context_window`，决策 46 / 111）。无可用 provider（FakeAgent /
        // 纯代码场景）时跳过分档——不臆造窗口；provider 存在但窗口未登记则显式失败。
        let capacity = model_context_window(ctx.store, ctx.task, cursor, ctx.stage_cfg)
            .await?
            .map(|model_window| {
                estimate_context_capacity(model_window, &system, &user, ctx.settings)
            });

        let plan = RequestPlan {
            system,
            user,
            hash,
            tools,
            capacity,
            temperature: ctx.stage_cfg.and_then(|c| c.temperature),
            max_tokens: ctx.stage_cfg.and_then(|c| c.max_tokens),
            provider_id: ctx.task.model_override.clone(),
            task_id: ctx.task.id.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt: ctx.attempt,
            keep_recent_rounds: ctx.settings.keep_recent_rounds,
            conversation_max_tokens: ctx.settings.conversation_max_tokens,
        };

        // 组装期唯一能判的超限：静态两段已超硬限——messages 压到 0 也还在限之上，
        // 没有进轮循环的必要。带 plan 回去（形状保证）：编排侧照样先落快照与会话行。
        if let Some(capacity) = plan.capacity {
            let estimate = count_tokens(&plan.system) + count_tokens(&plan.user);
            if over_hard_limit(estimate, capacity) {
                return Ok(Prepared::Overflow {
                    plan,
                    facts: OverflowFacts {
                        estimate,
                        hard_limit: capacity.hard_limit,
                    },
                });
            }
        }
        Ok(Prepared::Ready(plan))
    }

    /// prompt 快照（会话行的原文留痕；hash 是索引、原文是权威）。
    pub fn snapshot(&self) -> PromptSnapshot<'_> {
        PromptSnapshot {
            system: &self.system,
            user: &self.user,
        }
    }

    /// 每轮一扇（同步）：超软限**或**超硬底 → L3 按轮就地压缩；压缩后仍超硬限 → `Overflow`。
    ///
    /// `carried_len` = 续接锚点（决策 180：下标之前是载入历史，不得充当本轮锚点）。
    /// **入参是 `&mut`**：压缩会把载入历史压成摘要，旧下标随之作废——本函数把压缩后的
    /// 本轮起点写回去（票 03），调用方下一轮拿到的仍是指着锚点的下标。
    /// `capacity = None` 时软限不判（决策 110），硬底照判（票 03：不看 provider 脸色）。
    /// 越界是**值**，翻译（会话行 + `Pending(context_overflow)`）在编排侧——决策 245
    /// 「门吃落点不吃原因」。
    pub fn check_budget(
        &self,
        messages: &mut Vec<Message>,
        carried_len: &mut usize,
    ) -> BudgetCheck {
        // 算术在 `agent::context`（决策 291 / 票 foreman-unbounded 06）：值班长的轮内
        // 压缩与这里必须是同一份——两处各写一份就会各到各的线。
        let estimate = |msgs: &[Message]| {
            crate::agent::context::estimate_messages_tokens(&self.system, &self.user, msgs)
        };
        let tokens = estimate(messages);
        let chars = transcript_chars(messages);
        if !should_compact_with_floor(tokens, self.capacity, self.conversation_max_tokens) {
            return BudgetCheck::Ok { compacted: None };
        }
        // L3：规则化按轮压缩（不调 LLM，§12.13.3 规则表）
        let before = messages.len();
        let outcome = compact_messages_from(messages, self.keep_recent_rounds, *carried_len);
        let after = outcome.messages.len();
        *messages = outcome.messages;
        *carried_len = outcome.current_start;
        // 压缩发生时有可观测记录（票面要求）；触发源记下来——硬底触发说明软限那条线
        // 没拦住（登记虚高 / 无 provider），正是票 03 要观测的现场。字符读数保留作对照
        // （long-run-budget 票 02：触发已改 token 口径，chars 不再参与判定）。
        let trigger = if tokens > self.conversation_max_tokens {
            "token_floor"
        } else {
            "soft_limit"
        };
        tracing::info!(
            task = %self.task_id,
            stage = %self.stage,
            node = %self.node,
            trigger,
            tokens,
            chars,
            floor = self.conversation_max_tokens,
            before,
            after,
            compacted = outcome.compacted_messages,
            "上下文超线（软限或硬底），已按轮压缩（§12.13 L3 / 票 03）"
        );

        if let Some(capacity) = self.capacity {
            if over_hard_limit(estimate(messages), capacity) {
                // L4 判定（决策 105 / 148⑦ / 154）：压缩后仍超硬限。**绝不能放行继续跑**——
                // 那会让超硬限的节点无限循环。「挂 pending」的 warn 与构造在翻译侧（落点归构造者）。
                return BudgetCheck::Overflow {
                    estimate: estimate(messages),
                    hard_limit: capacity.hard_limit,
                };
            }
        }
        BudgetCheck::Ok {
            compacted: Some(outcome.compacted_messages),
        }
    }

    /// **无条件**按轮压缩（决策 295 / 票 10）：provider 报超窗时用。
    ///
    /// 与 [`Self::check_budget`] 的差别只有一处：**不问软限那条线**（也不问硬底）。
    /// 理由与值班长那侧一字不差（票 06(c)）——那次报错本身就是「算术低估了」的证据，
    /// 按同一条线再判一次只会得出「还没到线」然后原样再发一遍。返回压掉的条数，
    /// **0 = 没有可压的**（都在 keep 窗口里，回执就是极限）——调用方据此判
    /// 「压不动了，报错才是诚实的」。`carried_len` 同 [`Self::check_budget`]：就地写回。
    pub fn force_compact(&self, messages: &mut Vec<Message>, carried_len: &mut usize) -> usize {
        let outcome = compact_messages_from(messages, self.keep_recent_rounds, *carried_len);
        *messages = outcome.messages;
        *carried_len = outcome.current_start;
        outcome.compacted_messages
    }

    /// 把计划拼成一次 LLM 请求（身份戳与采样参数都在计划里，轮内不变）。
    pub fn request(&self, messages: &[Message], run: Option<RunContext>) -> LlmRequest {
        LlmRequest {
            stage: self.stage,
            node: self.node,
            attempt: self.attempt,
            system_prompt: self.system.clone(),
            user_prompt: self.user.clone(),
            messages: messages.to_vec(),
            tools: self.tools.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            provider_id: self.provider_id.clone(),
            run,
            // 节点的挂死由调度器的心跳判定收口（决策 64/66/88），不走流上 watchdog。
            idle_timeout_sec: None,
        }
    }
}

/// 五追加段的取数（首轮为空不渲染；打回/复检态先落库或落文件、再由重入渲染）。
///
/// 三处注入文件的读**全走有界阻塞读**（决策 302，票 01）：任务目录可配置在受保护路径下
/// （`AGENTPIPELINE_HOME` 指到 `~/Documents/...` 就是本机今天的实际形态）。
async fn load_segments(ctx: &AttemptCtx<'_>) -> Result<PromptSegments> {
    let home = ctx.store.home();
    Ok(PromptSegments {
        gate_recheck: gate_recheck_segment(ctx.store, ctx.task, ctx.cursor).await?,
        backtrack_feedback: architect_reentry_segment(
            home,
            &ctx.task.id,
            ctx.cursor.stage,
            ctx.cursor.node,
            "backtrack-feedback.md",
        )
        .await,
        user_input: if ctx.user_input_as_turn {
            // 决策 279：补充输入已作为 user turn 在转录末尾，不再渲染进首条消息——
            // 首条消息逐字不变，prompt cache 的前缀承诺从「run 内」延伸到「resume」。
            None
        } else {
            architect_reentry_segment(
                home,
                &ctx.task.id,
                ctx.cursor.stage,
                ctx.cursor.node,
                "user-input.md",
            )
            .await
        },
        review_required_changes: if ctx.review_rework_as_turn {
            // 决策 387：打回反馈已作为 user turn 在转录末尾，不再渲染进首条消息——
            // 首条消息逐字不变，prompt cache 的前缀承诺从「run 内」延伸到「resume」。
            None
        } else {
            review_required_changes_segment(ctx.store, ctx.task, ctx.cursor).await?
        },
        retry_feedback: architect_reentry_segment(
            home,
            &ctx.task.id,
            ctx.cursor.stage,
            ctx.cursor.node,
            "retry-feedback.md",
        )
        .await,
        zero_commit: zero_commit_facts_segment(home, &ctx.task.id, ctx.cursor).await,
        undeclared_changes: undeclared_changes_facts_segment(home, &ctx.task.id, ctx.cursor).await,
        // 票 04：简报文本已由编排侧组好（`AttemptCtx.continuation_brief`），本函数只把它
        // 搬进段表（渲染在 `build_user_prompt`）。放在段表末尾：它是「这一轮从哪起跑」的
        // 交代，读在其余反馈段之后更顺。
        continuation_brief: ctx.continuation_brief.clone(),
    })
}

/// 组装 §10.3 / G12 模板变量。上游产出取已登记的 stage output 路径；
/// 缺失时回退到任务目录下的规范文件名（agent 自行探测存在性，决策 115）。
async fn template_vars(
    store: &Store,
    task: &Task,
    project: &Project,
    worktree: &str,
    task_dir: &str,
) -> Result<TemplateVars> {
    let framework = project.test_framework.as_deref();
    let stored_path = |output: Option<crate::types::StageOutput>, default: &str| {
        output
            .map(|o| format!("{task_dir}/{}", o.file_path))
            .unwrap_or_else(|| format!("{task_dir}/{default}"))
    };
    let design = store
        .get_stage_output(&task.id, Stage::ArchitectDesign, OUTPUT_DESIGN_DOC)
        .await?;
    let dev = store
        .get_stage_output(&task.id, Stage::DevelopDesign, OUTPUT_DEV_DOC)
        .await?;
    let scenarios = store
        .get_stage_output(&task.id, Stage::TestDesign, OUTPUT_TEST_SCENARIOS)
        .await?;
    let code_changes = store
        .stage_output_metadata(&task.id, Stage::Develop, OUTPUT_CODE_CHANGES)
        .await?;
    let (changed, unit_tests) = code_changes_lists(code_changes.as_ref());
    Ok(TemplateVars {
        test_command: test_command_for(framework),
        test_file_convention: test_file_convention(framework).to_string(),
        test_framework: framework.unwrap_or("未知").to_string(),
        worktree_path: worktree.to_string(),
        task_dir: task_dir.to_string(),
        task_title: task.title.clone(),
        task_description: task.description.clone(),
        design_doc_path: stored_path(design, "design.md"),
        dev_doc_path: stored_path(dev, "dev-plan.md"),
        test_scenarios_path: stored_path(scenarios, "test-scenarios.md"),
        changed_files: changed,
        unit_test_files: unit_tests,
    })
}

/// decision 85 / 109：test.execute 被 merge 测试闸门打回时，prompt 注入闸门完整日志
/// + 失败用例，让 agent 重新判定 `failure_cause`。首轮（无闸门失败）为空不渲染。
async fn gate_recheck_segment(
    store: &Store,
    task: &Task,
    cursor: &NodeCursor,
) -> Result<Option<String>> {
    if cursor.stage != Stage::Test || cursor.node != Node::Execute {
        return Ok(None);
    }
    let Some(merge) = store.merge_metadata(&task.id).await? else {
        return Ok(None);
    };
    if merge.gate != Some(crate::types::Gate::Fail)
        || !matches!(
            merge.gate_failure_kind,
            Some(crate::types::GateFailureKind::Test) | None
        )
    {
        return Ok(None);
    }
    let mut out = String::new();
    // 决策 109 / 票 09：读闸门命令的**完整日志**（含被首尾预览裁掉的中间行），
    // 不再是 head/tail 预览。日志文件路径按 merge 闸门所在 stage 命名（覆盖写入可重入）；
    // 读取不到时回退 metadata 里的预览并显式标注（不静默）。
    let gate_log_path = store.home().task_file(
        &task.id,
        &format!("gate-output-{}.log", Stage::Merge.as_str()),
    );
    // 闸门日志可能很大（决策 109 注入上限 120k 字符）：读进阻塞池，超界按「完整日志
    // 不可读」处置——下面那条显式回退（退 metadata 预览 + 标注）本来就是为这种情形写的。
    let full_log = match bounded_read::read_to_string("gate_log", &gate_log_path).await {
        Offloaded::Done(Ok(log)) => Some(log),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty());
    match full_log {
        Some(log) => {
            out.push_str("### 闸门失败完整日志\n");
            out.push_str(&truncate_gate_log(&log, GATE_INJECTION_LIMIT));
            out.push('\n');
        }
        None => {
            // 完整日志缺失（异常路径）：退回 metadata 预览并显式说明，**不静默**
            if let Some(log) = merge
                .gate_failure_output
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                out.push_str("### 闸门失败输出（完整日志不可读，以下为首尾预览）\n");
                out.push_str(log.trim());
                out.push('\n');
            }
        }
    }
    if let Some(meta) = store
        .stage_output_metadata(&task.id, Stage::Test, OUTPUT_TEST_REPORT)
        .await?
    {
        if let Ok(t) = serde_json::from_value::<crate::types::TestResult>(meta) {
            if !t.failures.is_empty() {
                out.push_str("### 上一轮失败用例\n");
                for f in &t.failures {
                    out.push_str(&format!(
                        "- {}：{}（{}）\n",
                        f.test_name,
                        f.error_message,
                        match f.failure_cause {
                            crate::types::FailureCause::TestIssue => "test_issue",
                            crate::types::FailureCause::CodeIssue => "code_issue",
                        }
                    ));
                }
            }
        }
    }
    if out.trim().is_empty() {
        return Ok(None);
    }
    out.push_str(
        "\n请基于以上闸门输出，为每个失败用例重新标注 failure_cause（test_issue / code_issue）。",
    );
    Ok(Some(out))
}

/// 决策 391：`develop_code_gate` 的零提交守卫判定分支自有提交数为 0 时，把事实与落提交
/// 指令落成任务目录下的 `zero-commit-facts.md`；develop.execute 重入时读回注入。放行或
/// 申报 `no_changes` 时该文件被守卫清除，于是段自然为空——「首轮为空不渲染」与「已落提交
/// 不渲染」是同一支。读不到 / 读超界（决策 302）一律不渲染。
async fn zero_commit_facts_segment(
    home: &Home,
    task_id: &str,
    cursor: &NodeCursor,
) -> Option<String> {
    if cursor.stage != Stage::Develop || cursor.node != Node::Execute {
        return None;
    }
    let path = home.task_file(task_id, super::executor::ZERO_COMMIT_FACTS_FILE);
    match bounded_read::read_to_string("zero_commit_facts", &path).await {
        Offloaded::Done(Ok(content)) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

/// 决策 397：`develop_code_gate` 的申报比对判定有漏报时，把事实与补申报指令落成
/// 任务目录下的 `undeclared-changes-facts.md`；develop.execute 重入时读回注入。
/// 放行 / 补申报通过 / 读数降级 / 申报 `no_changes` 时该文件被守卫清除，于是段自然
/// 为空——「首轮为空不渲染」与「已补申报不渲染」是同一支。读不到 / 读超界
/// （决策 302）一律不渲染。
async fn undeclared_changes_facts_segment(
    home: &Home,
    task_id: &str,
    cursor: &NodeCursor,
) -> Option<String> {
    if cursor.stage != Stage::Develop || cursor.node != Node::Execute {
        return None;
    }
    let path = home.task_file(task_id, super::executor::UNDECLARED_CHANGES_FACTS_FILE);
    match bounded_read::read_to_string("undeclared_changes_facts", &path).await {
        Offloaded::Done(Ok(content)) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

/// 决策 133 / pipeline-spec §6：review 打回循环中，develop.execute 重入的 user prompt
/// 追加 review 的必须修改项。
///
/// 只在 `(Develop, Execute)` 渲染；修改项取自评审产出（review 报告 metadata），
/// 不重新推断。首轮进入 develop 时尚无评审产出 → 不渲染；评审不通过但
/// `required_changes` 为空 → 显式降级为「本次无结构化修改项」，不静默留空段。
async fn review_required_changes_segment(
    store: &Store,
    task: &Task,
    cursor: &NodeCursor,
) -> Result<Option<String>> {
    if cursor.stage != Stage::Develop || cursor.node != Node::Execute {
        return Ok(None);
    }
    let Some(meta) = store
        .stage_output_metadata(&task.id, Stage::Review, OUTPUT_REVIEW_REPORT)
        .await?
    else {
        return Ok(None); // 首轮：review 尚未执行
    };
    let Ok(review) = serde_json::from_value::<crate::types::ReviewResult>(meta) else {
        return Ok(None);
    };
    // 评审通过 → 不是打回，不渲染该段
    if review.approved {
        return Ok(None);
    }
    let source = review
        .review_report_path
        .as_deref()
        .unwrap_or("review-report.md");
    let mut out = format!("来源：评审报告 `{source}`（review 判定不通过）\n");
    if review.required_changes.is_empty() {
        // 显式降级：不让 agent 误以为「没有要求」
        out.push_str("本次无结构化修改项——请阅读上述评审报告，按其文字结论修改。\n");
    } else {
        out.push_str("本轮必须修改：\n");
        for change in &review.required_changes {
            out.push_str(&format!("- {} `{}`\n", change.action.label(), change.path));
        }
    }
    // 决策 406：本段是**无转录时的降级通道**（决策 387 之后真打回走 turn），红线两处
    // 共挂一份——只挂 turn 会在降级路径上丢掉这条纪律。
    out.push_str(crate::types::REVIEW_REWORK_DISCIPLINE);
    Ok(Some(out))
}

/// 解析本次 LLM 调用的模型上下文窗口（决策 110 / 票 04）。
///
/// 窗口来源是 `providers.context_window`（决策 46 / 111：随 provider 行存在一起，
/// 前端可改）——「注册表」就是 provider 表本身。解析顺序与生产适配器一致
/// （决策 129 四级：节点级 > 任务覆盖 > 阶段配置 > 系统默认首个 enabled）。
///
/// 返回 `Ok(None)` 仅表示**根本没有可用 provider**（测试注入 FakeAgent / 纯代码场景）——
/// 此时没有窗口可估，跳过 L0/L3/L4 分档，**不臆造一个窗口值**。
/// 一旦解析到 provider 但窗口未登记（为 0），**显式失败**，不静默取默认（决策 110）。
async fn model_context_window(
    store: &Store,
    task: &Task,
    cursor: &NodeCursor,
    stage_cfg: Option<&StageConfig>,
) -> Result<Option<usize>> {
    let node_override =
        crate::storage::catalog::node_provider_override(stage_cfg, cursor.node.as_str());
    let providers = store.load_providers().await?;
    let resolved = crate::storage::catalog::resolve_provider_id(
        node_override.as_deref(),
        task.model_override.as_deref(),
        stage_cfg,
        providers.iter().find(|p| p.enabled).map(|p| p.id.as_str()),
    );
    let Some(provider_id) = resolved else {
        return Ok(None);
    };
    let provider = providers
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| {
            Error::Config(format!(
                "provider {provider_id} 未注册，无法确定模型上下文窗口"
            ))
        })?;
    if provider.context_window == 0 {
        return Err(Error::Config(format!(
            "provider {}（{}）未登记 context_window，无法进行 L0 容量预估",
            provider.id, provider.model
        )));
    }
    Ok(Some(provider.context_window as usize))
}

/// 有效工具定义：基线并集（G6）内且 v1 已实现的内置工具 + `submit_metadata`
/// schema 工具（决策 38：与校验同源，不可移除）。声明了未知工具名**报错**（不静默丢弃）。
///
/// `Skill`（决策 172③，票 06）**不进 [`crate::agent::baseline::BASELINE_MANDATORY_TOOLS`]**：
/// 它由阶段声明启用。但名字态与目录态技能的存在意义就是「正文由 `Skill` 工具按需拉取」
/// ——若阶段声明了任一非全文态技能却没声明 `Skill`，那批技能就是断腿的指针。因此这里给
/// 一条**自动放行**：只要有技能不处于全文态，就补上 `Skill` 工具定义，不要求用户在两处
/// 各配一遍。
///
/// `stage` / `node` 只进报错信息（阶段 + 节点）——故意传枚举而不是拼好的字符串：
/// 这条分支在正常路径上不可达（同上），不该为它每轮多分配一个 `String`。
fn tool_defs(
    kind: AgentNodeKind,
    declared: &[String],
    skills: &[crate::agent::skills::ResolvedSkill],
    env_mode: crate::types::EnvMode,
    stage: crate::types::Stage,
    node: crate::types::Node,
) -> crate::Result<Vec<ToolDef>> {
    use crate::agent::skills::SkillRender;

    let needs_skill_tool = skills
        .iter()
        .any(|s| matches!(s.render, SkillRender::Name | SkillRender::Catalogue { .. }));
    let mut defs: Vec<ToolDef> = Vec::new();
    // `deny` 档**连广告都不给**（决策 206）：环境层工具直接从 tool 定义里摘掉，
    // 而不是等模型发出来再拒一次。执行点那一道仍在（[`crate::agent::tools::ToolExecutor`]），
    // 两道都留是因为它们挡的不是同一种东西：这里挡「模型看见了一个不该给的选项」，
    // 那里挡「模型无视定义硬发」。
    //
    // 这是全仓压过强制基线的地方之一（[`crate::agent::baseline::BASELINE_MANDATORY_TOOLS`]
    // 里含 `write_file` / `run_command`）——压的方向只有收紧一种，故它是安全的：阶段配置
    // 动不了它，只有全机档位可以。另一处在下面决策 396 的阶段过滤（review / test-design
    // 摘 `run_command`）：那不是系统级设置，而是按阶段的固定契约，两处判据独立。
    // 判据只有一处（`agent::tools::denied_by_tier`）：值班长那一侧的广告集与这里问的是同一个
    // 问题，两处各写一份谓词的后果是「一侧摘掉了、另一侧还广告着」这种只能靠现象定位的漂移。
    let denied = |name: &str| crate::agent::tools::denied_by_tier(name, env_mode);
    for name in effective_tools(declared) {
        if name == crate::agent::catalog::SUBMIT_METADATA {
            continue; // 最后以 schema 形式追加（决策 38：与校验同源，不走目录表）
        }
        if denied(&name) {
            continue;
        }
        // 阶段工具禁用（决策 396）：review / test-design 的模板全链路零依赖
        // `run_command`（探查核实），def 层不给——去诱惑。执行层兜底仍在
        // （`tools::run_command` 的阶段禁用闸），两道都留的理由与上面 `denied`
        // 相同：这里挡「模型看见了一个不该给的选项」，那里挡「模型无视定义硬发」。
        if name == crate::agent::catalog::RUN_COMMAND
            && matches!(
                stage,
                crate::types::Stage::Review | crate::types::Stage::TestDesign
            )
        {
            continue;
        }
        // 扩展工具（决策 172③，票 08）：不是内置工具，但**已实现**且由阶段声明启用
        // （设计阶段未配置时，`effective_declared_tools` 已默认给一条声明——决策 400）。
        // 不认这一条的话，声明了 `spawn_sub_agent` 会在下面被当作「未实现」丢弃 + warn，
        // 于是声明与生效之间静默断开。
        //
        // `deny` 档的摘除**不在这里重复判**：上面那次 `denied` 已经把它挡下了
        // （它与环境层其余工具同归一层）——同一支里判两遍，第二遍永远走不到。
        if name == crate::agent::SPAWN_SUB_AGENT_TOOL {
            defs.push(spawn_sub_agent_tool_def());
            continue;
        }
        // v1 不认识的名字 = 配置错误，**拒绝**（决策 154 的后续票）。
        //
        // 这条分支在启动路径上不可达：`PUT /stage-configs` 与启动校验（`validate_startup`）
        // 用的是同一个判据 [`crate::agent::client::is_known_tool_name`]，那个名字根本写不进库。
        // 留着它是为了**不给同一个错误第二种处置**——手工改库绕过校验时，这里报错（报文也与
        // 校验同源，见 `client::unknown_tools_message`）而不是「静默丢弃 + 一条 warn」：
        // 后者会让「配置写了却没生效」只能靠翻日志发现。
        if !crate::agent::client::is_known_tool_name(&name) {
            return Err(crate::Error::Config(
                crate::agent::client::unknown_tools_message(
                    &format!("阶段 {stage} 节点 {node}"),
                    std::slice::from_ref(&name),
                ),
            ));
        }
        // 决策 353：广告从目录表取（name + description + 参数 schema 一张表）——
        // 空壳广告（`description: String::new()` + `{"type":"object"}`）已退场，
        // `edit_file` 的 old_text/new_text 契约从此在广告出的接口里。
        // 走到这里的名字 ⊆ 目录表（上面的已知集检查 + submit_metadata 已 continue），
        // 这条 else 是与上一条分支同姿态的不可达兜底。
        let Some(def) = crate::agent::catalog::def_for(&name) else {
            return Err(crate::Error::Config(format!(
                "阶段 {stage} 节点 {node}：工具 {name} 没有目录行（agent::catalog）"
            )));
        };
        defs.push(def);
    }
    // 有名字态 / 目录态技能 → 自动带上 `Skill`（渐进披露的按需拉取入口）。
    // 定义从目录表取（决策 353）。
    if needs_skill_tool && !denied(SKILL_TOOL) && !defs.iter().any(|d| d.name == SKILL_TOOL) {
        defs.push(
            crate::agent::catalog::def_for(SKILL_TOOL)
                .expect("Skill 的目录行必须存在（agent::catalog 冻结断言钉住）"),
        );
    }
    defs.push(submit_metadata_tool_for(kind));
    Ok(defs)
}

/// 每个 agent 节点的 `submit_metadata` 工具定义——schema 由 `schemars` 从元数据类型生成
/// （决策 38：与校验同源，不会漂移）。
///
/// 抽成函数是为了让**模板 ↔ schema 一致性测试**（票 03）复用这**同一张映射**：测试若另写
/// 一份 `kind → 类型` 的表，就等于把"唯一口径"又拆成两份，漂移只是从模板挪到了测试里。
pub(crate) fn submit_metadata_tool_for(kind: AgentNodeKind) -> ToolDef {
    match kind {
        AgentNodeKind::ValidateInput => {
            submit_metadata_tool::<crate::types::ValidateInputMetadata>("提交输入充分性判定")
        }
        AgentNodeKind::ArchitectExecute => {
            submit_metadata_tool::<crate::types::ArchitectExecuteMetadata>("提交架构设计元数据")
        }
        AgentNodeKind::DevelopDesignExecute => {
            submit_metadata_tool::<crate::types::DevelopDesignMetadata>("提交开发计划元数据")
        }
        AgentNodeKind::TestDesignExecute => {
            submit_metadata_tool::<crate::types::TestDesignMetadata>("提交测试场景元数据")
        }
        AgentNodeKind::DesignValidateOutput => {
            submit_metadata_tool::<crate::types::ValidateOutputMetadata>("提交产出校验结论")
        }
        AgentNodeKind::DevelopExecute => {
            submit_metadata_tool::<crate::types::CodeChanges>("提交代码变更元数据")
        }
        AgentNodeKind::ReviewExecute => {
            submit_metadata_tool::<crate::types::ReviewResult>("提交评审结论")
        }
        AgentNodeKind::TestExecute => {
            submit_metadata_tool::<crate::types::TestResult>("提交测试结果元数据")
        }
    }
}

/// `spawn_sub_agent` 工具的 tool 定义（决策 172③，票 08）。
///
/// 描述里明说**只读**：让模型知道子代理能做什么，才不会派它去写文件或跑命令而白等一轮。
/// 子代理只能 read_file / list_dir，不能写文件或执行命令，也不再派子代理。
/// 适合「读很多文件、只要结论」的场景——原文留在子代理上下文，父上下文只收摘要。
/// 它**不在**目录表（决策 353 收的是 8 个内置工具；这是扩展工具——设计阶段未配置时
/// 默认声明、其余阶段显式声明，决策 400 / 172③）。
fn spawn_sub_agent_tool_def() -> ToolDef {
    ToolDef {
        name: crate::agent::SPAWN_SUB_AGENT_TOOL.to_string(),
        description: "派生一个只读子代理处理可分解的检索子任务，返回摘要。\
                      子代理只能 read_file / list_dir，不能写文件或执行命令，也不再派子代理。\
                      适合「读很多文件、只要结论」的场景——原文留在子代理上下文，父上下文只收摘要。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "子任务描述：要检索什么、要回答什么问题、需要什么形态的结论"
                }
            },
            "required": ["task"]
        }),
    }
}

// ─────────────────────── prompt 组装辅助（票 12：§10.3 / G3 / G6 / G12）───────────────────────

/// G12 工作目录行（system 的「工作目录」段与 user 的「环境路径」段共用，防漂移）。
pub(crate) fn workdirs_line(worktree: &str, task_dir: &str) -> String {
    format!("worktree：{worktree}\n任务目录：{task_dir}")
}

/// architect-design 重入时从任务目录注入的反馈文件段（首轮为空不渲染）。
///
/// 三处注入同构、只差文件名（决策 126 / 79 / 138），共用此读取器：
/// - `backtrack-feedback.md`：sync-check backtrack 的双方 blockers（决策 126）；
/// - `user-input.md`：`info_insufficient` 的用户补充输入（决策 79 / 票 08）；
/// - `retry-feedback.md`：develop / test 重试耗尽回架构设计的失败摘要（决策 138）。
///
/// 读不到（含读超界，决策 302）一律不渲染——「首轮为空不渲染」与「读不到不渲染」是同一支。
async fn architect_reentry_segment(
    home: &Home,
    task_id: &str,
    stage: Stage,
    node: Node,
    file: &str,
) -> Option<String> {
    if stage != Stage::ArchitectDesign || !matches!(node, Node::ValidateInput | Node::Execute) {
        return None;
    }
    match bounded_read::read_to_string("task_file", &home.task_file(task_id, file)).await {
        Offloaded::Done(Ok(content)) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

/// persona 解析（决策 7 / §10.6.3）：`stage_configs.persona_path` 显式指定优先，
/// 其次 `prompts/{stage}/{node}.md` 用户覆盖，最后内嵌 §10.3 模板；
/// `persona_append` 追加为额外指令段。
///
/// 两处读都走有界阻塞读（决策 302，票 01）：`persona_path` 相对 home 解析、home 又可
/// 被指到受保护路径；`prompts/` 覆盖目录同理。读超界与「读不到」在 `persona_path` 这支
/// 上仍报 `Config`（它的契约是「显式指定的路径必须可读」），只是报文里如实写超界。
async fn resolve_stage_persona(
    home: &Home,
    stage_cfg: Option<&StageConfig>,
    stage: Stage,
    node: Node,
) -> Result<String> {
    let embedded = system_template(stage, node);
    let mut content = match stage_cfg.and_then(|c| c.persona_path.as_deref()) {
        Some(path) => {
            // 相对路径按 home 根解析；绝对路径原样使用
            let p = home.root().join(path);
            match bounded_read::read_to_string("persona_path", &p).await {
                Offloaded::Done(Ok(read)) => {
                    if read.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "阶段 {stage} 的 persona_path 内容为空：{}",
                            p.display()
                        )));
                    }
                    read
                }
                Offloaded::Done(Err(e)) => {
                    return Err(Error::Config(format!(
                        "阶段 {stage} 的 persona_path 不可读：{}（{e}）",
                        p.display()
                    )))
                }
                Offloaded::Panicked(msg) => {
                    return Err(Error::Config(format!(
                        "阶段 {stage} 的 persona_path 不可读：{}（{msg}）",
                        p.display()
                    )))
                }
                Offloaded::Stuck => {
                    return Err(Error::Config(format!(
                        "阶段 {stage} 的 persona_path 不可读：{}（读超界 {}s，仍挂在系统调用里）",
                        p.display(),
                        bounded_read::BOUNDED_READ_SEC
                    )))
                }
            }
        }
        None => {
            resolve_persona(&home.prompts_dir(), stage, node, embedded)
                .await
                .content
        }
    };
    if let Some(append) = stage_cfg.and_then(|c| c.persona_append.as_deref()) {
        if !append.trim().is_empty() {
            content.push_str(&format!("\n\n{append}"));
        }
    }
    Ok(content)
}

/// 阶段配置里的字符串数组字段（tools_json / skills_json）。
pub(super) fn json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `spawn_sub_agent` 默认开启的阶段（决策 400）：三个设计阶段——它们的工作本身就是
/// 「读一堆文件做检索再提炼结论」，正是只读子代理的定位场景（决策 172③）。其余阶段
/// 维持「显式声明才启用」的原样；值班长侧不在此列（foreman 的工具清单有意不给它
/// 子代理运行器，见 `pipeline/foreman/catalog.rs`）。
pub(super) const SUBAGENT_DEFAULT_STAGES: [crate::types::Stage; 3] = [
    crate::types::Stage::ArchitectDesign,
    crate::types::Stage::DevelopDesign,
    crate::types::Stage::TestDesign,
];

/// 阶段工具声明的**生效值**：`tools_json` 显式配置的原样生效（含 `[]` = 显式关闭）；
/// **没配过**（`None`）时，设计阶段默认带一条 `spawn_sub_agent` 声明，其余阶段为空
/// （决策 400）。
///
/// 广告侧（[`RequestPlan::assemble`]）与注入侧（`model_invoke` 的子代理运行器）都从
/// 这里取值——两处若各算一份，迟早漂成「模型看得见一个调了就被拒的工具」（票 01 的
/// 「广告集与白名单同源」在同一件事上的延续）。
pub(super) fn effective_declared_tools(
    stage: crate::types::Stage,
    tools_json: Option<&serde_json::Value>,
) -> Vec<String> {
    match tools_json {
        Some(value) => json_string_list(Some(value)),
        None if SUBAGENT_DEFAULT_STAGES.contains(&stage) => {
            vec![crate::agent::SPAWN_SUB_AGENT_TOOL.to_string()]
        }
        None => Vec::new(),
    }
}

/// 从 code_changes stage output 提取变更文件 / 单元测试文件列表（每行一个路径）。
/// 缺失时给降级说明（决策 115 / 133：评审与测试模板需容忍上游阶段被跳过）。
fn code_changes_lists(value: Option<&serde_json::Value>) -> (String, String) {
    const MISSING: &str = "（缺失：本任务跳过了对应阶段，按决策 115 降级处理）";
    let Some(value) = value else {
        return (MISSING.into(), MISSING.into());
    };
    let changes: Option<crate::types::CodeChanges> = serde_json::from_value(value.clone()).ok();
    let join = |specs: &[crate::types::FileChangeSpec]| {
        if specs.is_empty() {
            MISSING.to_string()
        } else {
            specs
                .iter()
                .map(|s| s.path.clone())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    match changes {
        Some(c) => (join(&c.changed_files), join(&c.unit_test_files)),
        None => (MISSING.into(), MISSING.into()),
    }
}

/// 测试框架 → 系统闸门命令的文件命名惯例（§6）。
fn test_file_convention(framework: Option<&str>) -> &'static str {
    match framework {
        Some("pytest") => "tests/test_*.py",
        Some("npm") | Some("node") => "**/*.test.ts",
        _ => "tests/*_test.rs",
    }
}

/// 闸门复检注入体积上界（决策 109 / 票 09）：约 120k 字符。
///
/// 决策要求注入 `kanban_node_commands` 的**完整日志**；但注入必须有上界，否则一份
/// 超大闸门日志会挤爆复检 prompt。超限时**显式**保留首尾并写明省略了多少字符
/// （并给出完整日志路径），**不静默回退到预览**——票据明确禁止无声降级。
const GATE_INJECTION_LIMIT: usize = 120_000;

/// 显式截断：保留首尾并标注省略量（不静默丢内容）。
fn truncate_gate_log(log: &str, limit: usize) -> String {
    if log.chars().count() <= limit {
        return log.to_string();
    }
    // 按字符切（日志可能含中文），避免在 UTF-8 边界截断
    let head_n = limit * 2 / 3;
    let tail_n = limit - head_n;
    let chars: Vec<char> = log.chars().collect();
    let head: String = chars[..head_n].iter().collect();
    let tail: String = chars[chars.len() - tail_n..].iter().collect();
    format!(
        "{head}\n\n...[闸门日志超长：已省略中间 {} 字符；完整日志见上方 stdout_path]...\n\n{tail}",
        chars.len() - limit
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;

    use crate::agent::prompts::{BASELINE_PREAMBLE, FORMAT_RULES};
    use crate::clock::SystemClock;
    use crate::storage::tasks::NewTask;
    use crate::types::{CursorStatus, EnvMode, Provider};

    // ───────────────── 既有用例随搬（一条不删，决策 249 Q3）─────────────────

    #[tokio::test]
    async fn backtrack_feedback_only_injected_for_architect_reentry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path());
        std::fs::create_dir_all(home.task_dir("t1")).unwrap();

        // 首轮：反馈文件不存在 → 不渲染（决策 126「首轮为空不渲染」）
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "backtrack-feedback.md"
            )
            .await,
            None
        );

        std::fs::write(
            home.task_file("t1", "backtrack-feedback.md"),
            "dev blockers：[\"缺少数据流定义\"]\n",
        )
        .unwrap();
        assert!(architect_reentry_segment(
            &home,
            "t1",
            Stage::ArchitectDesign,
            Node::ValidateInput,
            "backtrack-feedback.md"
        )
        .await
        .is_some());
        assert!(architect_reentry_segment(
            &home,
            "t1",
            Stage::ArchitectDesign,
            Node::Execute,
            "backtrack-feedback.md"
        )
        .await
        .is_some());

        // 决策 126 的注入范围只有 validate_input / execute
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateOutput,
                "backtrack-feedback.md"
            )
            .await,
            None
        );
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::Develop,
                Node::Execute,
                "backtrack-feedback.md"
            )
            .await,
            None
        );

        // 空文件（纯空白）不渲染
        std::fs::write(home.task_file("t1", "backtrack-feedback.md"), "  \n").unwrap();
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "backtrack-feedback.md"
            )
            .await,
            None
        );
    }

    #[test]
    fn gate_log_under_limit_is_returned_verbatim() {
        // 未超限：完整保留（含中间行，票据要求读全文而非首尾预览）
        let log = "line1\n".repeat(10);
        assert_eq!(truncate_gate_log(&log, 1000), log);
    }

    #[test]
    fn gate_log_over_limit_truncates_with_explicit_notice() {
        // 超限：显式截断并标注省略量，保留首尾，**不静默**丢内容
        let mid = "MIDDLE_OMITTED_MARKER\n";
        let log = format!("HEAD\n{}{}", mid.repeat(50), "TAIL\n");
        let out = truncate_gate_log(&log, 100);
        assert!(out.starts_with("HEAD"), "保留首部");
        assert!(out.ends_with("TAIL\n"), "保留尾部");
        assert!(out.contains("闸门日志超长"), "应有显式省略标注");
        assert!(out.contains("已省略中间"), "标注应写明省略量");
        assert!(out.len() < log.len(), "截断后应变短");
        // 中间行确实被省略（这正是首尾预览会丢掉的那段）
        assert!(!out.contains(&mid.repeat(50)));
    }

    #[test]
    fn gate_log_truncation_is_char_boundary_safe() {
        // 中文字符不得被按字节切开（否则输出非法 UTF-8 / 乱码）
        let log = "中".repeat(500);
        let out = truncate_gate_log(&log, 100);
        assert!(out.is_char_boundary(out.len()));
        assert!(out.chars().all(|c| c == '中'
            || ".\n[闸门日志超长：已省略中间 400 字符；完整日志见上方 stdout_path]".contains(c)));
    }

    /// 决策 353：广告集从目录表取——描述非空、schema 带真实参数形状；`submit_metadata`
    /// 的 schema 仍由 Rust 结构体派生（决策 38 同源），目录占位不得泄漏进广告。
    #[test]
    fn tool_defs_serves_catalog_specs_and_keeps_metadata_schema_derived() {
        let (stage, node) = (crate::types::Stage::Develop, crate::types::Node::Execute);
        let defs = tool_defs(
            AgentNodeKind::DevelopExecute,
            &[],
            &[],
            crate::types::EnvMode::Auto,
            stage,
            node,
        )
        .unwrap();
        let read_file = defs
            .iter()
            .find(|d| d.name == crate::agent::catalog::READ_FILE)
            .unwrap();
        assert!(!read_file.description.trim().is_empty(), "空壳广告已退场");
        assert!(
            read_file.parameters.get("properties").is_some(),
            "广告出的 schema 须带参数形状：{:?}",
            read_file.parameters
        );
        let md = defs
            .iter()
            .find(|d| d.name == crate::agent::catalog::SUBMIT_METADATA)
            .unwrap();
        assert_eq!(
            md,
            &crate::agent::submit_metadata_tool::<crate::types::CodeChanges>("提交代码变更元数据"),
            "submit_metadata 的 schema 只能来自结构体派生（决策 38）"
        );
    }

    /// 决策 396：review / test-design 的 def 层不给 `run_command`——模板全链路零依赖
    /// （探查核实），广告出去就是诱惑。执行层兜底在 `tools::run_command`（两层挡的不是
    /// 同一种东西，理由见 tool_defs 内注释）。其余阶段不动：develop 靠它落提交（391）。
    #[test]
    fn run_command_is_not_advertised_for_review_and_test_design() {
        use crate::types::{Node as N, Stage as S};
        let denied_cases = [
            (AgentNodeKind::ReviewExecute, S::Review, N::Execute),
            (
                AgentNodeKind::ValidateInput,
                S::TestDesign,
                N::ValidateInput,
            ),
            (AgentNodeKind::TestDesignExecute, S::TestDesign, N::Execute),
            (
                AgentNodeKind::DesignValidateOutput,
                S::TestDesign,
                N::ValidateOutput,
            ),
        ];
        for (kind, stage, node) in denied_cases {
            let defs = tool_defs(kind, &[], &[], EnvMode::Auto, stage, node).unwrap();
            assert!(
                !defs
                    .iter()
                    .any(|d| d.name == crate::agent::catalog::RUN_COMMAND),
                "{stage}/{node} 不得广告 run_command"
            );
            assert!(
                defs.iter()
                    .any(|d| d.name == crate::agent::catalog::SUBMIT_METADATA),
                "{stage}/{node} 的 submit_metadata schema 必须仍在"
            );
        }
        // 其余阶段照旧：develop 的提交契约依赖 run_command（决策 391）
        let defs = tool_defs(
            AgentNodeKind::DevelopExecute,
            &[],
            &[],
            EnvMode::Auto,
            S::Develop,
            N::Execute,
        )
        .unwrap();
        assert!(
            defs.iter()
                .any(|d| d.name == crate::agent::catalog::RUN_COMMAND),
            "develop 必须仍能拿到 run_command（决策 391 的提交契约）"
        );
        // 双层一致性（票 02 断言 3）：对全部 12 个 agent 节点，`run_command` 的
        // 「def 层广告 ⇔ 非（review / test-design）」——两层状态必须互为镜像，
        // 否则会出现「广告了但必拒」（浪费 attempts）或「没广告却能跑」。
        let stage_denied = |s: S| matches!(s, S::Review | S::TestDesign);
        for (stage, node, kind) in [
            (
                S::ArchitectDesign,
                N::ValidateInput,
                AgentNodeKind::ValidateInput,
            ),
            (
                S::DevelopDesign,
                N::ValidateInput,
                AgentNodeKind::ValidateInput,
            ),
            (
                S::TestDesign,
                N::ValidateInput,
                AgentNodeKind::ValidateInput,
            ),
            (
                S::ArchitectDesign,
                N::Execute,
                AgentNodeKind::ArchitectExecute,
            ),
            (
                S::DevelopDesign,
                N::Execute,
                AgentNodeKind::DevelopDesignExecute,
            ),
            (S::TestDesign, N::Execute, AgentNodeKind::TestDesignExecute),
            (
                S::ArchitectDesign,
                N::ValidateOutput,
                AgentNodeKind::DesignValidateOutput,
            ),
            (
                S::DevelopDesign,
                N::ValidateOutput,
                AgentNodeKind::DesignValidateOutput,
            ),
            (
                S::TestDesign,
                N::ValidateOutput,
                AgentNodeKind::DesignValidateOutput,
            ),
            (S::Develop, N::Execute, AgentNodeKind::DevelopExecute),
            (S::Review, N::Execute, AgentNodeKind::ReviewExecute),
            (S::Test, N::Execute, AgentNodeKind::TestExecute),
        ] {
            let advertised = tool_defs(kind, &[], &[], EnvMode::Auto, stage, node)
                .unwrap()
                .iter()
                .any(|d| d.name == crate::agent::catalog::RUN_COMMAND);
            assert_eq!(
                advertised,
                !stage_denied(stage),
                "{stage}/{node}：def 层广告与执行层禁用必须互为镜像"
            );
        }
    }

    /// 决策 154 的后续票：`tool_defs` 对未知工具名**报错**，不再「静默丢弃 + 一条 warn」。
    ///
    /// 这条分支在启动路径上不可达（配置根本写不进来），留着是为了**不给同一个错误第二种
    /// 处置**——手工改库绕过校验时行为与写入时一致：拒绝。故这条用例同时钉住报文形状。
    #[test]
    fn tool_defs_rejects_unknown_names_and_accepts_the_known_set() {
        let (stage, node) = (crate::types::Stage::Develop, crate::types::Node::Execute);
        let err = tool_defs(
            AgentNodeKind::DevelopExecute,
            &["read_file".to_string(), "web_search".to_string()],
            &[],
            crate::types::EnvMode::Auto,
            stage,
            node,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("web_search"), "{err}");
        assert!(
            err.contains("阶段 develop 节点 execute"),
            "报文须定位到阶段 + 节点：{err}"
        );
        assert!(err.contains("v1 已知工具集"), "{err}");

        // 已知集（含扩展工具）照旧出表：`spawn_sub_agent` 走它自己的定义分支
        let defs = tool_defs(
            AgentNodeKind::DevelopExecute,
            &["read_file".to_string(), "spawn_sub_agent".to_string()],
            &[],
            crate::types::EnvMode::Auto,
            stage,
            node,
        )
        .unwrap();
        assert!(defs.iter().any(|d| d.name == "read_file"));
        assert!(defs.iter().any(|d| d.name == "spawn_sub_agent"));
        assert!(defs.iter().any(|d| d.name == "submit_metadata"));
    }

    /// 决策 400：生效声明清单——显式配置原样（`[]` = 显式关闭），没配过时设计阶段
    /// 默认带 `spawn_sub_agent`，其余阶段为空。广告与注入两侧共用这一份。
    #[test]
    fn effective_declared_tools_defaults_subagent_for_design_stages() {
        use crate::types::Stage;
        let explicit = serde_json::json!(["read_file"]);
        assert_eq!(
            effective_declared_tools(Stage::ArchitectDesign, Some(&explicit)),
            vec!["read_file".to_string()],
            "显式配置原样生效"
        );
        assert_eq!(
            effective_declared_tools(Stage::ArchitectDesign, Some(&serde_json::json!([]))),
            Vec::<String>::new(),
            "显式 [] = 显式关闭"
        );
        for stage in [
            Stage::ArchitectDesign,
            Stage::DevelopDesign,
            Stage::TestDesign,
        ] {
            assert_eq!(
                effective_declared_tools(stage, None),
                vec![crate::agent::SPAWN_SUB_AGENT_TOOL.to_string()],
                "{stage:?} 未配置时默认带 spawn_sub_agent"
            );
        }
        for stage in [Stage::Develop, Stage::Review, Stage::Test, Stage::Init] {
            assert_eq!(
                effective_declared_tools(stage, None),
                Vec::<String>::new(),
                "{stage:?} 未配置时没有默认扩展工具"
            );
        }
    }

    // ───────────────── 新增窄测试：D 桶（组装）+ E 桶（预算），只做加法 ─────────────────

    /// 共同基座：临时 home + 全量迁移的临时库 + 播种的项目/任务。不经 `Executor`、
    /// 不真调 LLM、不建 git 仓（票 01 验收的三个「不」）。
    ///
    /// **只用仓内原语**：testkit 依赖回 core，在 in-crate 单测里会同名类型两个实例
    /// （`--cfg test` 与普通编译各一份），类型对不上——集成层才是 testkit 的家。
    async fn base() -> (
        tempfile::TempDir,
        Home,
        Store,
        Task,
        Project,
        Settings,
        NodeCursor,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path().join("home"));
        let store = Store::open(home.clone(), Arc::new(SystemClock))
            .await
            .unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = Project {
            id: "p1".into(),
            name: "proj".into(),
            local_path: repo.display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: None,
            lint_command: None,
            agents_md_path: None,
            created_at: Utc::now(),
        };
        store.create_project(&project).await.unwrap();
        let task = store
            .create_task(&NewTask::new("t1", "任务 t1", "p1"))
            .await
            .unwrap();
        let settings = Settings::default();
        let cursor = cursor_of(Stage::ArchitectDesign, Node::ValidateInput);
        (tmp, home, store, task, project, settings, cursor)
    }

    fn cursor_of(stage: Stage, node: Node) -> NodeCursor {
        NodeCursor {
            cursor_id: "c1".into(),
            task_id: "t1".into(),
            branch: NodeCursor::BRANCH_MAIN.into(),
            stage,
            node,
            status: CursorStatus::Active,
            validate_attempts: 0,
            skipped_to_join: false,
            pending_reason: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn empty_stage_cfg(stage: Stage) -> StageConfig {
        StageConfig {
            stage: stage.as_str().to_string(),
            provider_id: None,
            temperature: None,
            max_tokens: None,
            persona_path: None,
            persona_append: None,
            tools_json: None,
            skills_json: None,
            idle_timeout_sec: None,
            max_duration_sec: None,
            node_overrides_json: None,
            env_mode: None,
            max_rounds: None,
            watch_token_budget: None,
            updated_at: Utc::now(),
        }
    }

    fn ctx<'a>(
        store: &'a Store,
        settings: &'a Settings,
        task: &'a Task,
        project: &'a Project,
        cursor: &'a NodeCursor,
        stage_cfg: Option<&'a StageConfig>,
        kind: AgentNodeKind,
    ) -> AttemptCtx<'a> {
        AttemptCtx {
            store,
            settings,
            task,
            project,
            cursor,
            stage_cfg,
            attempt: 1,
            kind,
            user_input_as_turn: false,
            review_rework_as_turn: false,
            continuation_brief: None,
            // 组装层大部分用例与授权无关：这一份是「还没探过」（判不出来就不拦人）。
            // 缺授权那条路自己在 `a_denied_disk_access_snapshot_fails_fast` 里置值。
            disk_access: crate::agent::disk_access::DiskAccessState::NotProbed,
        }
    }

    fn write_skill(home: &Home, name: &str, body: &str) {
        let dir = home.skills_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试技能\n---\n{body}\n"),
        )
        .unwrap();
    }

    fn provider(context_window: u32) -> Provider {
        Provider {
            id: "prov-1".into(),
            vendor: "test".into(),
            model: "fake-model".into(),
            context_window,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    async fn assemble_ok(c: AttemptCtx<'_>) -> RequestPlan {
        match RequestPlan::assemble(c).await.unwrap() {
            Prepared::Ready(plan) => plan,
            Prepared::Overflow { .. } => panic!("expected Ready, got Overflow"),
        }
    }

    #[tokio::test]
    async fn assemble_lays_the_system_prompt_out_in_golden_order() {
        // D 桶·golden 序（§10.3 / G12 / G3）：不是测 prompts.rs 的渲染本身（那有 insta），
        // 是测 **assemble 这条路**把五段按序拼进 system——段的取数、persona 解析、
        // 技能解析都可能把顺序弄错，只有过 interface 才测得到。
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        std::fs::write(
            Path::new(&project.local_path).join("AGENTS.md"),
            "AGENTS_MARKER_XYZ",
        )
        .unwrap();
        std::fs::write(home.root().join("persona.md"), "PERSONA_MARKER_XYZ").unwrap();
        write_skill(&home, "skill-a", "SKILL_BODY_XYZ");
        let mut cfg = empty_stage_cfg(Stage::ArchitectDesign);
        cfg.persona_path = Some("persona.md".into());
        cfg.persona_append = Some("APPEND_MARKER_XYZ".into());
        cfg.skills_json = Some(serde_json::json!(["skill-a"]));

        let plan = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&cfg),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        let s = &plan.system;
        let idx = |needle: &str| s.find(needle).unwrap_or_else(|| panic!("缺少 {needle}"));
        assert!(idx(BASELINE_PREAMBLE) < idx("## 工作目录"), "基线前言第一");
        assert!(
            idx("## 工作目录") < idx("AGENTS_MARKER_XYZ"),
            "工作目录 → AGENTS.md"
        );
        assert!(
            idx("AGENTS_MARKER_XYZ") < idx("PERSONA_MARKER_XYZ"),
            "AGENTS → persona"
        );
        assert!(
            idx("PERSONA_MARKER_XYZ") < idx("APPEND_MARKER_XYZ"),
            "persona 正文 → persona_append"
        );
        assert!(
            idx("APPEND_MARKER_XYZ") < idx("## 已启用技能"),
            "persona → 技能段"
        );
        assert!(
            idx("## 已启用技能") < idx("SKILL_BODY_XYZ"),
            "技能段头 → 技能正文"
        );
        assert!(
            idx("SKILL_BODY_XYZ") < idx(FORMAT_RULES),
            "技能 → 格式规则收尾"
        );
    }

    /// 票 05：组装是**缓存友好的稳定前缀**——同一份输入两次组装逐字节相同，工具顺序固定。
    ///
    /// provider 的前缀缓存只认「前缀逐字节相同」（106 的实测命中率与计费见
    /// `.scratch/106-stability/cache-findings.md`）。这里任何一处轻微抖动都会让整段已缓存
    /// 前缀作废，而它在功能用例里完全看不出来——换个 `HashMap` 收集顺序就够了。故本用例
    /// 盯的是**确定性**本身：同一输入必须给出同一串字节、同一个 hash、同一份工具序列
    /// （基线序 + 声明序 + schema 工具收尾，决策 38）。
    #[tokio::test]
    async fn assemble_freezes_a_byte_stable_head_and_a_fixed_tool_order() {
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        let cfg = empty_stage_cfg(Stage::ArchitectDesign);
        let build = || {
            ctx(
                &store,
                &settings,
                &task,
                &project,
                &cursor,
                Some(&cfg),
                AgentNodeKind::ValidateInput,
            )
        };
        let first = assemble_ok(build()).await;
        let second = assemble_ok(build()).await;

        assert_eq!(
            first.system, second.system,
            "system 逐字节稳定（缓存前缀的第一段）"
        );
        assert_eq!(first.user, second.user, "user 逐字节稳定");
        assert_eq!(first.hash, second.hash, "hash 是 system 的索引，同样稳定");

        let names = |plan: &RequestPlan| -> Vec<String> {
            plan.tools.iter().map(|t| t.name.clone()).collect()
        };
        let first_names = names(&first);
        assert_eq!(first_names, names(&second), "工具定义的顺序固定");

        // 基线工具的**顺序就是常量里的顺序**（决策 45）：任何收集方式的抖动
        // （`HashMap` / 并行 gather）都会在这里露出来。architect-design 未配置
        // `tools_json`，按决策 400 默认多一条 `spawn_sub_agent` 声明——恰好把
        // 「声明序跟在基线序之后」这一半也钉住。
        let mandated: Vec<&str> = crate::agent::client::MANDATORY_TOOLS
            .iter()
            .copied()
            .filter(|n| *n != crate::agent::catalog::SUBMIT_METADATA)
            .collect();
        assert_eq!(
            first_names.last().map(String::as_str),
            Some(crate::agent::catalog::SUBMIT_METADATA),
            "schema 工具收尾（决策 38）：{first_names:?}"
        );
        let body = &first_names[..first_names.len() - 1];
        let (baseline, declared) = body.split_at(mandated.len());
        assert_eq!(
            baseline.iter().map(String::as_str).collect::<Vec<_>>(),
            mandated,
            "基线工具按常量序在前：{first_names:?}"
        );
        assert_eq!(
            declared.iter().map(String::as_str).collect::<Vec<_>>(),
            ["spawn_sub_agent"],
            "architect-design 未配置时默认声明 spawn_sub_agent（决策 400）：{first_names:?}"
        );
    }

    #[tokio::test]
    async fn full_text_skill_body_changes_the_hash_but_name_only_does_not() {
        // 决策 170 / 211②：hash 对全文态技能正文敏感、名字态钝感——原文是权威，
        // hash 只是索引；两者都经 interface 出口拿。
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        write_skill(&home, "skill-a", "BODY_V1");
        let mut full = empty_stage_cfg(Stage::ArchitectDesign);
        full.skills_json = Some(serde_json::json!(["skill-a"]));
        let h1 = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&full),
            AgentNodeKind::ValidateInput,
        ))
        .await
        .hash;
        write_skill(&home, "skill-a", "BODY_V2");
        let h2 = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&full),
            AgentNodeKind::ValidateInput,
        ))
        .await
        .hash;
        assert_ne!(h1, h2, "全文态：正文变了 hash 必须变");

        let mut named = empty_stage_cfg(Stage::ArchitectDesign);
        named.skills_json = Some(serde_json::json!([{"name": "skill-a", "mode": "name"}]));
        let n1 = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&named),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        write_skill(&home, "skill-a", "BODY_V3");
        let n2 = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&named),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert_eq!(n1.hash, n2.hash, "名字态：正文不进 prompt，hash 钝感");
        assert!(n2.system.contains("- skill-a"), "名字态仍列名字");
        assert!(!n2.system.contains("BODY_V"), "名字态正文不进 prompt");
    }

    #[tokio::test]
    async fn reentry_segments_stay_unrendered_until_the_feedback_lands() {
        // 五追加段（首轮为空不渲染，决策 126/79/138）：过 interface 测——段的取数与
        // 门（stage/node 判定）都在组装里，单测读文件的那份测不到门。
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        let first = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(!first.user.contains("## 上游回溯反馈"), "首轮为空不渲染");
        assert!(!first.user.contains("## 用户补充输入"), "首轮为空不渲染");

        std::fs::create_dir_all(home.task_dir("t1")).unwrap();
        std::fs::write(
            home.task_file("t1", "backtrack-feedback.md"),
            "dev blockers：[\"缺数据流\"]",
        )
        .unwrap();
        let second = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(second.user.contains("## 上游回溯反馈"), "打回后渲染");
        assert!(second.user.contains("缺数据流"), "内容来自反馈文件");

        // 门：同名文件在 architect 之外的节点不渲染（决策 126 注入范围）
        let other = cursor_of(Stage::Develop, Node::Execute);
        let develop = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &other,
            None,
            AgentNodeKind::DevelopExecute,
        ))
        .await;
        assert!(
            !develop.user.contains("## 上游回溯反馈"),
            "范围只有 architect 重入"
        );
    }

    #[tokio::test]
    async fn user_input_segment_yields_to_the_transcript_turn_when_appended() {
        // 决策 279：补充输入已进转录（user turn，validate_input 续接）→ segment 停止
        // 渲染——同一段话不出现两遍，首条消息也不因 resume 而变；未进转录（首轮 /
        // execute 等）→ segment 照旧渲染。
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        std::fs::create_dir_all(home.task_dir("t1")).unwrap();
        std::fs::write(
            home.task_file("t1", "user-input.md"),
            "补充的事实：部署在 k8s",
        )
        .unwrap();

        let rendered = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(
            rendered.user.contains("## 用户补充输入"),
            "未进转录时照旧渲染"
        );

        let suppressed = assemble_ok(AttemptCtx {
            user_input_as_turn: true,
            review_rework_as_turn: false,
            ..ctx(
                &store,
                &settings,
                &task,
                &project,
                &cursor,
                None,
                AgentNodeKind::ValidateInput,
            )
        })
        .await;
        assert!(
            !suppressed.user.contains("用户补充输入"),
            "进转录后 segment 让位：{}",
            suppressed.user
        );
    }

    #[tokio::test]
    async fn node_scoped_skills_inject_only_the_declaring_node() {
        // 决策 172④：节点级技能只进声明节点的 prompt；同阶段另一节点只在目录态见到名字
        // （正文不进）。集成层有全栈版（integration/executor.rs），这是 interface 窄版。
        let (_tmp, home, store, task, project, settings, _) = base().await;
        write_skill(&home, "skill-a", "NODE_BODY_XYZ");
        let mut cfg = empty_stage_cfg(Stage::ArchitectDesign);
        cfg.node_overrides_json = Some(serde_json::json!({"execute": {"skills": ["skill-a"]}}));

        let exec_cursor = cursor_of(Stage::ArchitectDesign, Node::Execute);
        let with_skill = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &exec_cursor,
            Some(&cfg),
            AgentNodeKind::ArchitectExecute,
        ))
        .await;
        assert!(
            with_skill.system.contains("NODE_BODY_XYZ"),
            "声明节点注入正文"
        );

        let vi_cursor = cursor_of(Stage::ArchitectDesign, Node::ValidateInput);
        let without = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &vi_cursor,
            Some(&cfg),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(
            !without.system.contains("NODE_BODY_XYZ"),
            "未声明节点不注入正文"
        );
        assert!(
            without.system.contains("- skill-a"),
            "未声明节点仍在目录态可见"
        );
    }

    #[tokio::test]
    async fn persona_path_and_append_flow_into_the_system_prompt() {
        // 决策 7 / §10.6.3：persona_path 显式指定优先 + persona_append 追加。
        // 与 golden 序用例分开：这条测**内容**（路径解析、非空校验之外的正路）。
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        std::fs::write(home.root().join("persona.md"), "PERSONA_BODY_ONLY").unwrap();
        let mut cfg = empty_stage_cfg(Stage::ArchitectDesign);
        cfg.persona_path = Some("persona.md".into());
        cfg.persona_append = Some("APPEND_ONLY".into());
        let plan = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&cfg),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(plan.system.contains("PERSONA_BODY_ONLY"));
        assert!(plan.system.contains("APPEND_ONLY"));
    }

    #[tokio::test]
    async fn persona_path_unreadable_is_a_config_error() {
        // 错误模式（票 01 §二）：persona_path 不可读 → Error::Config，不是静默回落。
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        let mut cfg = empty_stage_cfg(Stage::ArchitectDesign);
        cfg.persona_path = Some("no-such-persona.md".into());
        let err = RequestPlan::assemble(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&cfg),
            AgentNodeKind::ValidateInput,
        ))
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("persona_path 不可读"), "{err}");
    }

    #[test]
    fn deny_tier_strips_env_tool_ads_but_metadata_survives() {
        // 决策 206：deny 档连广告都不给（整层环境工具摘除），schema 工具不受影响。
        let (stage, node) = (Stage::Develop, Node::Execute);
        let denied = tool_defs(
            AgentNodeKind::DevelopExecute,
            &[],
            &[],
            EnvMode::Deny,
            stage,
            node,
        )
        .unwrap();
        assert!(
            !denied.iter().any(|d| d.name == "write_file"),
            "deny 档：环境层写工具连广告都不给"
        );
        assert!(
            !denied.iter().any(|d| d.name == "run_command"),
            "deny 档：run_command 摘除"
        );
        assert!(
            denied.iter().any(|d| d.name == "submit_metadata"),
            "schema 工具不受档位影响"
        );

        let allowed = tool_defs(
            AgentNodeKind::DevelopExecute,
            &[],
            &[],
            EnvMode::Auto,
            stage,
            node,
        )
        .unwrap();
        assert!(
            allowed.iter().any(|d| d.name == "write_file"),
            "auto 档：基线强制工具照常广告"
        );
        assert!(allowed.iter().any(|d| d.name == "run_command"));
    }

    #[tokio::test]
    async fn assemble_flags_overflow_with_the_plan_still_in_hand() {
        // 甲的 Overflow 臂（决策 249 · Q13）：静态两段已超硬限 → 组装期直接判出，
        // 但 plan 必须还在手上——先落快照与会话行再收口是形状保证（决策 180 退出路径）。
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        // 窗口 50：任何真实 system prompt 都超过它的硬限，而 provider 已解析——
        // 决策 110 的「已解析且窗口非 0」正路。
        store.upsert_provider(&provider(50)).await.unwrap();
        match RequestPlan::assemble(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await
        .unwrap()
        {
            Prepared::Overflow { plan, facts } => {
                assert!(
                    facts.estimate > facts.hard_limit,
                    "estimate {} 应超硬限 {}",
                    facts.estimate,
                    facts.hard_limit
                );
                assert!(plan.capacity.is_some());
                assert!(!plan.system.is_empty(), "plan 还在：原文可落快照");
                assert!(!plan.hash.is_empty());
            }
            Prepared::Ready(_) => panic!("窗口 50 应判 Overflow"),
        }
    }

    /// E 桶基座：经 interface 组装两遍——先探静态两段的 token 数（`ContextCapacity.reserved_*`
    /// 就是它的读数），再把软/硬限钉在「静态 + margin」上。边界因此与静态长度解耦：
    /// `estimate` = 静态 + 消息、软限 = 静态 + soft_margin，判越界 ⟺ 消息侧超 margin，
    /// 不靠猜默认 persona 有多长。
    async fn budget_plan(soft_margin: usize, hard_margin: usize) -> RequestPlan {
        let (_tmp, _home, store, task, project, mut settings, cursor) = base().await;
        store.upsert_provider(&provider(1_000_000)).await.unwrap();
        let probe = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        let cap = probe.capacity.expect("provider 已给窗口");
        let s = cap.reserved_system + cap.reserved_user;
        let w = 1_000_000.0_f64;
        settings.context_soft_limit_ratio = (s + soft_margin) as f64 / w;
        settings.context_hard_limit_ratio = (s + hard_margin) as f64 / w;
        assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await
    }

    /// 与既有 L3 用例同形的对话：[system, user] + N 轮 assistant 大文本。
    fn conversation(rounds: usize, tokens_each: usize) -> Vec<Message> {
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        for i in 0..rounds {
            messages.push(Message::assistant(
                Some(format!("round {i} {}", "x".repeat(tokens_each * 4))),
                vec![],
            ));
        }
        messages
    }

    #[tokio::test]
    async fn check_budget_passes_under_the_soft_limit() {
        // 顺带钉 L0 的字段出口：reserved_* 就是静态两段的 token 读数（margin 基座靠它）。
        let plan = budget_plan(50_000, 60_000).await;
        let cap = plan.capacity.unwrap();
        assert_eq!(cap.reserved_system, count_tokens(&plan.system));
        assert_eq!(cap.reserved_user, count_tokens(&plan.user));
        let mut messages = conversation(1, 10);
        let len_before = messages.len();
        match plan.check_budget(&mut messages, &mut 0) {
            BudgetCheck::Ok { compacted } => assert_eq!(compacted, None, "未触发压缩"),
            other => panic!("软限之下应 Ok：{other:?}"),
        }
        assert_eq!(messages.len(), len_before, "没压缩就不该动 messages");
    }

    #[tokio::test]
    async fn check_budget_compacts_in_place_between_soft_and_hard() {
        // 软限 = 静态 + 2000、硬限 = 静态 + 60000：12 轮 ×1000 token 起手超软限；
        // L3 保留 system/user + 最近 5 轮 + 摘要后应回到硬限内 → Ok{Some(n)}，就地变短。
        let plan = budget_plan(2_000, 60_000).await;
        let mut messages = conversation(12, 1_000);
        let len_before = messages.len();
        match plan.check_budget(&mut messages, &mut 0) {
            BudgetCheck::Ok { compacted } => {
                let n = compacted.expect("超软限必触发压缩");
                assert!(n > 0, "compacted 应为压掉的条数，得到 {n}");
            }
            other => panic!("软硬之间应压缩后 Ok：{other:?}"),
        }
        assert!(messages.len() < len_before, "L3 就地压缩");
        assert_eq!(
            messages[0].content.as_deref(),
            Some("sys"),
            "system 段不动（§12.13.3 规则表）"
        );
    }

    #[tokio::test]
    async fn force_compact_ignores_the_soft_limit() {
        // 与 `check_budget` 的差别只有一处：**不问软限那条线**（票 10）。provider 报超窗时
        // 要用它——那次报错本身就是「算术低估了」的证据，按软限再判一次只会得出「还没到线」，
        // 然后把同一份放不下的转录原样再发一遍（正是这一票要停掉的那件事）。
        //
        // 软限给足 60_000：12 轮 ×1000 token 的对话在 `check_budget` 眼里还早。
        let plan = budget_plan(60_000, 80_000).await;
        let mut messages = conversation(12, 1_000);
        let len_before = messages.len();
        assert!(
            matches!(
                plan.check_budget(&mut messages.clone(), &mut 0),
                BudgetCheck::Ok { compacted: None }
            ),
            "前提：这份对话在软限之下，预算门不会动它"
        );
        let compacted = plan.force_compact(&mut messages, &mut 0);
        assert!(compacted > 0, "无条件压缩要压得动：得到 {compacted}");
        assert!(messages.len() < len_before, "L3 就地压缩");
        // 压不动时如实回 0：调用方据此判「报错才是诚实的」（超窗那一次调用的处置）。
        let mut tiny = conversation(1, 10);
        assert_eq!(plan.force_compact(&mut tiny, &mut 0), 0, "没有旧轮可压 → 0");
    }

    #[tokio::test]
    async fn check_budget_reports_overflow_when_compaction_cannot_get_under_the_hard_limit() {
        // 软限 = 静态 + 2000、硬限 = 静态 + 4000：12 轮 ×1000 token，压缩保留 5 轮
        // （≈5010）+ 摘要后仍 > 4000 → Overflow，读数是**压缩后**的 estimate。
        let plan = budget_plan(2_000, 4_000).await;
        let cap = plan.capacity.unwrap();
        let s = cap.reserved_system + cap.reserved_user;
        let mut messages = conversation(12, 1_000);
        match plan.check_budget(&mut messages, &mut 0) {
            BudgetCheck::Overflow {
                estimate,
                hard_limit,
            } => {
                assert!(
                    hard_limit.abs_diff(s + 4_000) <= 1,
                    "硬限 = 静态 + 4000（{hard_limit} vs {}）",
                    s + 4_000
                );
                assert!(
                    estimate > hard_limit,
                    "estimate {estimate} 应超 {hard_limit}"
                );
            }
            other => panic!("压缩后仍超硬限应 Overflow：{other:?}"),
        }
    }

    #[tokio::test]
    async fn check_budget_without_capacity_skips_the_soft_line_but_not_the_floor() {
        // 决策 110：无 provider → capacity=None → 软限不判（不臆造窗口）。
        // 票 03 修订：硬底照判——压缩是本地规则算术，不需要窗口数字背书。
        // long-run-budget 票 02：硬底改 token 口径。这份对话 2 万 token，
        // 在 30 万 token 的硬底之下 → 不动。
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        let plan = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(plan.capacity.is_none());
        let mut messages = conversation(10, 2_000);
        let len_before = messages.len();
        let mut carried_len = 0;
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => assert_eq!(compacted, None),
            other => panic!("硬底之下应恒 Ok：{other:?}"),
        }
        assert_eq!(messages.len(), len_before, "没到线就不该动 messages");
    }

    #[tokio::test]
    async fn check_budget_compacts_without_capacity_when_tokens_exceed_the_floor() {
        // 票 03（long-run-budget 票 02 改 token 口径）：无 provider（FakeAgent / 纯代码
        // 场景）也拦——12 轮 × 10 万 token = 120 万 token，超过 30 万 token 的硬底
        // → 就地压缩，锚点下标跟着搬到 1（锚点 user 紧跟 system）。
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        let plan = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            None,
            AgentNodeKind::ValidateInput,
        ))
        .await;
        assert!(plan.capacity.is_none(), "前提：无 provider");
        let mut messages = conversation(12, 100_000);
        let len_before = messages.len();
        let mut carried_len = 0;
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => {
                let n = compacted.expect("超硬底必触发压缩");
                assert!(n > 0, "compacted 应为压掉的条数，得到 {n}");
            }
            other => panic!("超硬底应压缩后 Ok：{other:?}"),
        }
        assert!(messages.len() < len_before, "L3 就地压缩");
        assert_eq!(carried_len, 1, "锚点下标随压缩搬迁");
    }

    #[tokio::test]
    async fn token_floor_triggers_even_when_the_registered_window_is_inflated() {
        // ux-audit-3 的形状（票 03；long-run-budget 票 02 改 token 口径）：登记窗口虚高
        // → 软限跟着虚高 → 按软限那条线压缩永不触发。token 硬底是绝对数、不看登记的脸色：
        // 估算（≈60 万 token）远在软限（静态 + 100 万）之下，但过了硬底（缺省 30 万）
        // → 照压。
        let plan = budget_plan(1_000_000, 2_000_000).await;
        let cap = plan.capacity.unwrap();
        let mut messages = conversation(12, 50_000);
        let estimate =
            crate::agent::context::estimate_messages_tokens(&plan.system, &plan.user, &messages);
        assert!(
            estimate < cap.soft_limit,
            "前提自检：软限拦不住这份对话（{estimate} < {}）",
            cap.soft_limit
        );
        assert!(
            estimate > plan.conversation_max_tokens,
            "前提自检：token 估算才是触发源（{estimate} > {}）",
            plan.conversation_max_tokens
        );
        let mut carried_len = 0;
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => {
                assert!(compacted.expect("超硬底必触发压缩") > 0);
            }
            other => panic!("登记虚高时硬底必须兜住：{other:?}"),
        }
    }

    #[tokio::test]
    async fn check_budget_takes_the_current_round_as_anchor_not_the_loaded_history() {
        // 决策 180（票 13 必要条件三）：carried_len 之前是载入历史——上一轮的提问
        // **不得**充当「本轮第一条 user」这个锚点，否则 keep 预算被上一轮的提问占掉。
        // 形状照 context.rs 的既有锚点用例，这里过 interface（carried_len 是编排侧
        // 每轮要传对的那个值）。软限 = 静态 + 2000、硬限给足。
        let plan = budget_plan(2_000, 60_000).await;
        let mut messages = vec![
            Message::system("sys"),
            // 载入的历史：上一轮的提问（carried_len 之前）
            Message::user("上一轮的提问"),
            Message::assistant(Some("上一轮的回答".into()), vec![]),
            // 本轮起点
            Message::user("本轮提问"),
        ];
        for i in 0..12 {
            messages.push(Message::assistant(
                Some(format!("r{i} {}", "x".repeat(4_000))),
                vec![],
            ));
        }
        let mut carried_len = 3;
        let len_before = messages.len();
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => {
                assert!(compacted.unwrap_or(0) > 0, "仍应触发压缩");
            }
            other => panic!("应压缩后 Ok：{other:?}"),
        }
        assert!(messages.len() < len_before, "锚之后的轮次被压缩");
        assert_eq!(messages[0].content.as_deref(), Some("sys"), "system 段不动");
        assert_eq!(
            messages[1].content.as_deref(),
            Some("本轮提问"),
            "锚点须是本轮第一条 user——上一轮的提问没资格占位（决策 180）"
        );
        assert!(
            messages[2]
                .content
                .as_deref()
                .unwrap()
                .starts_with("[摘要]"),
            "历史上那条被压成摘要，紧跟锚点之后"
        );
    }

    #[tokio::test]
    async fn repeated_compaction_relocates_the_anchor_and_does_not_spin() {
        // 票 03：硬底生效后，同一 attempt 里连续多轮压缩成为常态。压缩把载入历史压成
        // 摘要后旧下标作废——若调用方不把新下标拿回去，下一次压缩会拿旧下标找锚点，
        // 把本轮真正的提问当成旧历史压掉（决策 180 锚点规则被击穿）。
        //
        // 形状即「下一 attempt 续接一份已压缩过的转录」：载入历史里带着上一轮压缩
        // 留下的 [摘要]，本轮提问在其后，12 轮 × 15 万字符（≈3.75 万 token）的大轮次
        // 两次撞 30 万 token 的硬底（long-run-budget 票 02 改 token 口径）。
        let plan = budget_plan(1_000_000, 2_000_000).await;
        let mut messages = vec![
            Message::system("sys"),
            // 载入的历史（carried_len = 4 之前）：上一轮的问答 + 它压缩留下的摘要
            Message::user("上一轮的提问"),
            Message::assistant(Some("上一轮的回答".into()), vec![]),
            Message::user("[摘要] 已完成的操作：\n- 已写入 notes-0.md"),
            // 本轮起点
            Message::user("本轮提问"),
        ];
        for i in 0..12 {
            messages.push(Message::assistant(
                Some(format!("r{i} {}", "x".repeat(150_000))),
                vec![],
            ));
        }
        let mut carried_len = 4;
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => assert!(compacted.unwrap_or(0) > 0, "第一次压缩"),
            other => panic!("第一次应压缩后 Ok：{other:?}"),
        }
        assert_eq!(carried_len, 1, "锚点下标随压缩搬迁");
        assert_eq!(
            messages[1].content.as_deref(),
            Some("本轮提问"),
            "锚点 user 原样保留"
        );
        let len_after_first = messages.len();
        let summaries_after_first = messages
            .iter()
            .filter(|m| m.content.as_deref().unwrap_or("").starts_with("[摘要]"))
            .count();
        assert_eq!(summaries_after_first, 1, "只应有一条摘要");

        // 第二次（模拟下一轮仍超硬底）：锚点不能被压掉，摘要不翻倍、转录不增长。
        match plan.check_budget(&mut messages, &mut carried_len) {
            BudgetCheck::Ok { compacted } => assert!(compacted.unwrap_or(0) > 0, "第二次仍压得动"),
            other => panic!("第二次应压缩后 Ok：{other:?}"),
        }
        assert_eq!(carried_len, 1, "锚点下标稳定");
        assert_eq!(
            messages[1].content.as_deref(),
            Some("本轮提问"),
            "连续压缩后锚点仍在本位——旧下标会把这一条压掉（票 03 修的正是这个）"
        );
        let summaries_after_second = messages
            .iter()
            .filter(|m| m.content.as_deref().unwrap_or("").starts_with("[摘要]"))
            .count();
        assert_eq!(summaries_after_second, 1, "摘要收敛为一条，不空转翻倍");
        assert!(
            messages.len() <= len_after_first,
            "转录不因反复压缩而增长：{} vs {len_after_first}",
            messages.len()
        );
    }

    // ───────────── 票 01（决策 302）：组装层的读全在阻塞池里，挂住也不再拖垮运行时 ─────────────

    /// **静态断言**：组装层里不再有直接的同步文件读。覆盖范围写死在这里。
    ///
    /// 为什么要有这一条：功能测试**看不出**这件事——环境不卡时，`std::fs::read_to_string`
    /// 与阻塞池里的读行为完全一样。而「只包住这次报错的那一处」是打地鼠（下一次卡的是
    /// persona 或技能根，症状一模一样），所以要靠一条读源码的断言把整层钉住。
    ///
    /// **不能靠删文件蒙混过关**：下面每条路径都必须存在且非空，正面清单还要求五处有界读
    /// 都在场——删函数、改名、把裸读挪回来，三者都会红。
    #[test]
    fn the_assembly_layer_has_no_direct_sync_reads() {
        let source = |rel: &str| -> String {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("覆盖范围内的文件必须存在（{}）：{e}", path.display()));
            assert!(!src.trim().is_empty(), "{} 是空文件", path.display());
            // 只看非测试段：测试里造 fixture 本来就该直接写文件。
            src.split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .to_string()
        };

        // 覆盖面①：组装层主体。
        let assembly = source("src/pipeline/model_request.rs");
        for banned in [
            "std::fs::read",
            "std::fs::read_dir",
            "File::open",
            // 技能两大读（目录扫描 + 正文）只准经有界入口——裸调用会把读留在 worker 上。
            "skills::resolve(",
            "skills::catalogue(",
        ] {
            assert!(
                !assembly.contains(banned),
                "组装层不得出现 `{banned}`：读要走 crate::agent::bounded_read"
            );
        }
        for anchor in [
            "resolve_bounded(",
            "catalogue_bounded(",
            "read_to_string(\"gate_log\"",
            "read_to_string(\"task_file\"",
            "read_to_string(\"persona_path\"",
        ] {
            assert!(assembly.contains(anchor), "组装层缺一处有界读：`{anchor}`");
        }

        // 覆盖面②：组装层调用的两个 prompt helper（项目指令文件 / persona）。
        let prompts = source("src/agent/prompts.rs");
        for banned in ["std::fs::read", "File::open"] {
            assert!(!prompts.contains(banned), "prompts.rs 不得出现 `{banned}`");
        }
        for anchor in [
            "bounded_read::read_to_string(\"agents_md\"",
            "bounded_read::read_to_string(\"persona\"",
        ] {
            assert!(
                prompts.contains(anchor),
                "prompts.rs 缺一处有界读：`{anchor}`"
            );
        }

        // 覆盖面③：技能那两处的有界版真的把扫描挪进了阻塞池。
        let skills = source("src/agent/skills.rs");
        for anchor in [
            "pub async fn resolve_bounded",
            "pub async fn catalogue_bounded",
            "bounded_read::run(\"skills_resolve\"",
            "bounded_read::run(\"skills_catalogue\"",
        ] {
            assert!(skills.contains(anchor), "skills.rs 缺 `{anchor}`");
        }
    }

    /// **真会挂住的读**（票 01 的核心用例）：命名管道充当项目指令文件。
    ///
    /// 为什么非造一个真挂住的：这次故障的形状就是「一次文件读挂在系统调用里 4 小时」，
    /// mock 一个慢读只会测到 mock。FIFO 的 `open()` 在写端出现之前**真的**不返回。
    /// 装置用的就是临时目录（与 `AGENTPIPELINE_HOME` 那条接缝同一个形状）——**不新增接缝**。
    ///
    /// 三条断言对着票面三条要求：挂住期间别的任务照常推进（worker 没被占死）、
    /// 挂住的读走完有界等待后被放弃而组装照常出结果、这次挂住被计入观测。
    #[tokio::test]
    // 计数器是进程级的，这条与 `bounded_read` 里那几条必须互斥跑；拿锁跨 await 是有意的。
    #[allow(clippy::await_holding_lock)]
    async fn a_hung_read_is_abandoned_within_the_bound_and_counted() {
        use std::io::Write as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let _guard = crate::agent::bounded_read::test_guard();
        let (_tmp, _home, store, task, project, settings, cursor) = base().await;
        // 时钟在**建好 fixture 之后**再暂停：`start_paused` 会把 sqlx 连接池的 acquire
        // 超时一起快进掉（实测 `Db(PoolTimedOut)` 炸在 `Store::open`）。
        tokio::time::pause();
        bounded_read::reset_stats();

        // 项目指令文件换成命名管道：`open()` 阻塞到有写端为止。
        let fifo = Path::new(&project.local_path).join("AGENTS.md");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo 不可用")
            .success());

        // 心跳：一条只让出执行权、不等任何计时器的任务——它跳一下 = 运行时还能调度别的东西。
        // （故意不用 `sleep`：那会把假时钟推起来，下面的读数就不确定了。）
        let beats = Arc::new(AtomicUsize::new(0));
        let beats_in_task = beats.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                beats_in_task.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
            }
        });

        let assembling = tokio::spawn(async move {
            let c = AttemptCtx {
                store: &store,
                settings: &settings,
                task: &task,
                project: &project,
                cursor: &cursor,
                stage_cfg: None,
                attempt: 1,
                kind: AgentNodeKind::ValidateInput,
                user_input_as_turn: false,
                review_rework_as_turn: false,
                continuation_brief: None,
                disk_access: crate::agent::disk_access::DiskAccessState::NotProbed,
            };
            RequestPlan::assemble(c).await
        });

        // 暖机：把组装推到管道那一次读上。时钟**一步不推**，所以这期间不可能有超时——
        // 真等 1ms 是给阻塞池里的几个快读（不存在的任务文件、空的技能根）收尾的时间。
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(1));
            tokio::task::yield_now().await;
        }
        assert_eq!(
            bounded_read::stats().stuck_total,
            0,
            "没到上界就不该记账（上界 {}s）",
            bounded_read::BOUNDED_READ_SEC
        );

        // ① 挂住期间运行时照常推进：心跳在跳，而组装还挂在那里。
        assert!(
            beats.load(Ordering::Relaxed) > 0,
            "worker 被占死了：心跳一次都没跳"
        );
        assert!(!assembling.is_finished(), "读还挂着，组装不该已经返回");

        // ② 越过上界：挂住的读被放弃，组装照常出结果（回落到既有的缺省上下文）。
        tokio::time::advance(Duration::from_secs(bounded_read::BOUNDED_READ_SEC + 1)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        let prepared = assembling.await.unwrap().unwrap();
        let Prepared::Ready(plan) = prepared else {
            panic!("没配 provider，不该判 Overflow")
        };
        assert!(
            plan.system.contains("本仓库无 AGENTS.md"),
            "读不到 → 既有的缺省上下文（读超界与读失败同一条降级路）"
        );

        // ③ 这次挂住被计入观测（票 07 的落点就在这个读数上）。
        let hung = bounded_read::stats();
        assert_eq!(hung.stuck_total, 1, "挂住的读要记账");
        assert_eq!(hung.stuck_now, 1, "线程还在系统调用里：卡着的就是 1");
        assert!(
            hung.longest_wait_ms >= bounded_read::BOUNDED_READ_SEC * 1000,
            "最长等待要如实记到上界：{}",
            hung.longest_wait_ms
        );

        heartbeat.abort();

        // 收尾：写端出现 → 那个读真的返回。「卡着的」跌回 0 是断言的一部分；
        // 顺带把阻塞线程放掉（不放，测试二进制退出时会等它，整轮挂住）。
        let mut writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        writer.write_all(b"x").unwrap();
        drop(writer);
        for _ in 0..200 {
            if bounded_read::stats().stuck_now == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            bounded_read::stats().stuck_now,
            0,
            "读返回后「卡着的」要跌回去"
        );
    }

    /// **缺授权时组装立刻失败，并给一句可操作的话**（决策 306，票 05）。
    ///
    /// 这是这次故障的第四层：真因是授权随重建失效（未签名 app 换 CDHash），而节点会白等
    /// 300 秒再失败——每一处读都挂在系统调用里。这里改成一进组装就判**启动时记下的快照**
    /// （值注入，`ctx.disk_access`），判词里带「若你刚刚已开启，请重启应用」。
    ///
    /// 反过来说：**判不出来（`NotProbed`）与有授权（`Granted`）都放行**——这个开关只拦
    /// 一种确定的状态。错误是 [`crate::Error::Config`]，沿 `model_invoke` 的 `?` 出去落进
    /// 执行体既有的错误路径 → 节点转 pending；而它发生在**任何模型调用之前**，
    /// 故不可能产生超时记账。
    #[tokio::test]
    async fn a_denied_disk_access_snapshot_fails_fast_with_the_next_step() {
        use crate::agent::disk_access::DiskAccessState;

        let (_tmp, home, store, task, mut project, settings, cursor) = base().await;
        // 判据是「缺授权 **且** 项目根在受保护的地方」（决策 306 的收窄）：把项目指到
        // 真实 `$HOME` 下的 `Documents`——**不必真存在**，缺授权那条路在读之前就返回了。
        let protected = std::env::var_os("HOME")
            .map(|h| {
                std::path::Path::new(&h)
                    .join("Documents")
                    .join("never-read-this")
                    .display()
                    .to_string()
            })
            .expect("用例要拿真实 $HOME 拼受保护路径");
        project.local_path = protected.clone();
        let ctx = |state: DiskAccessState| AttemptCtx {
            store: &store,
            settings: &settings,
            task: &task,
            project: &project,
            cursor: &cursor,
            stage_cfg: None,
            attempt: 1,
            kind: AgentNodeKind::ArchitectExecute,
            user_input_as_turn: false,
            review_rework_as_turn: false,
            continuation_brief: None,
            disk_access: state,
        };

        let err = RequestPlan::assemble(ctx(DiskAccessState::Denied))
            .await
            .expect_err("缺授权时不许继续组装");
        let text = err.to_string();
        assert!(text.contains("完全磁盘访问权限"), "{text}");
        assert!(
            text.contains("若你刚刚已开启，请重启应用"),
            "快照有滞后，必须说清「改了设置也要重启」：{text}"
        );
        // **类别也钉住**：它是 `Error::Config` ⇒ 沿执行体既有的错误路径走（游标转 pending），
        // 而它发生在任何模型调用之前 ⇒ 不可能产生超时记账。那两句话的端到端断言在
        // `executor::an_assembly_config_failure_pends_without_any_timeout_accounting`——
        // 两处合起来才是票 05 那条反向断言的完整链条。
        assert!(
            matches!(err, crate::Error::Config(_)),
            "缺授权是**配置类**失败（不是传输类、不该重试等一等）：{err:?}"
        );

        // 反向一：有授权（这一支永远静默）与判不出来（这一支不拦人）都要放行——
        // 缺授权那条路只拦**一种确定的状态**。
        for state in [DiskAccessState::Granted, DiskAccessState::NotProbed] {
            assert!(
                RequestPlan::assemble(ctx(state)).await.is_ok(),
                "{state:?} 不该被拦"
            );
        }
        // 反向二：**同样缺授权，但项目不在受保护目录下**（临时目录）→ 放行。
        // 这一条是收窄的那一半：不然没开完全磁盘访问的机器整个跑不动。
        let mut safe_project = project.clone();
        safe_project.local_path = protected.replace("/Documents/", "/tmp-");
        let safe_ctx = AttemptCtx {
            store: &store,
            settings: &settings,
            task: &task,
            project: &safe_project,
            cursor: &cursor,
            stage_cfg: None,
            attempt: 1,
            kind: AgentNodeKind::ArchitectExecute,
            user_input_as_turn: false,
            review_rework_as_turn: false,
            continuation_brief: None,
            disk_access: DiskAccessState::Denied,
        };
        assert!(
            RequestPlan::assemble(safe_ctx).await.is_ok(),
            "缺授权 + 项目在临时目录下不该被拦（那些读不需要授权）"
        );
        drop(home);
    }

    /// 正常路径**逐字不变**（决策 249 组装侧 golden 的先例）：
    /// 五处读挪进阻塞池之后，组装出的 system / user 两段与实际文件取数时**一字不差**。
    ///
    /// 归一化说明：worktree / 任务目录 / 家目录都落在临时目录里，故把 home 根前缀换成
    /// `<HOME>`；其余全部逐字进 golden（含 AGENTS.md 正文、persona 正文、技能三态渲染、
    /// 目录态、模板变量、格式规则）。
    #[tokio::test]
    async fn the_assembled_prompt_is_byte_identical_to_the_golden() {
        let (_tmp, home, store, task, project, settings, cursor) = base().await;
        std::fs::write(
            Path::new(&project.local_path).join("AGENTS.md"),
            "AGENTS_MARKER_XYZ\n第二行\n",
        )
        .unwrap();
        std::fs::write(home.root().join("persona.md"), "PERSONA_MARKER_XYZ\n").unwrap();
        write_skill(&home, "skill-a", "SKILL_BODY_XYZ");
        // 未声明的技能进目录态（名字 + 描述，正文不进 prompt）
        let catalogue_dir = home.skills_dir().join("skill-b");
        std::fs::create_dir_all(&catalogue_dir).unwrap();
        std::fs::write(
            catalogue_dir.join("SKILL.md"),
            "---\nname: skill-b\ndescription: 目录态描述\n---\n目录态正文不进 prompt\n",
        )
        .unwrap();
        let mut cfg = empty_stage_cfg(Stage::ArchitectDesign);
        cfg.persona_path = Some("persona.md".into());
        cfg.persona_append = Some("APPEND_MARKER_XYZ".into());
        cfg.skills_json = Some(serde_json::json!(["skill-a"]));

        let plan = assemble_ok(ctx(
            &store,
            &settings,
            &task,
            &project,
            &cursor,
            Some(&cfg),
            AgentNodeKind::ValidateInput,
        ))
        .await;
        let home_prefix = home.root().display().to_string();
        let norm = |s: &str| s.replace(&home_prefix, "<HOME>");
        insta::assert_snapshot!("assembled_system_prompt", norm(&plan.system));
        insta::assert_snapshot!("assembled_user_prompt", norm(&plan.user));
    }
}
