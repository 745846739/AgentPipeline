# 03: provider 配错后可理解、可恢复

**What to build:** 真实使用中最高频的失败是「模型或密钥配错」。当前行为链：
LLM 调用失败 → `Error::Llm("HTTP {status}：{body preview}")`（`crates/core/src/agent/providers/mod.rs:187`）
→ executor 在 `run_inner` 捕获 → `pend_cursor(RetryExhausted, node_error.to_string())`
（`crates/core/src/pipeline/executor.rs:203-216`）→ 任务挂起，面板显示 `reason.message`
（`PendingDossier.svelte:71`）。

也就是用户看到的是**原始错误串**（英文 HTTP 状态 + 供应商返回体片段），配上通用文案
「重试耗尽，需要用户介入」（`pending_message`，executor.rs:3227）。用户不知道
（a）是哪个环节失败（b）该去改什么（c）改完怎么继续。而且配置页**没有「测试连接」按钮**
（`SettingsProviders.svelte` / `ProviderForm.svelte` 均无），用户只能靠建任务来试错。

本票让配错**可理解、可恢复**：区分可归因的错误类别（鉴权失败 / 模型不存在 / 网络不可达 /
上下文窗口不匹配），给出可操作提示，并在配置侧提供一次连通性验证。

**Blocked by:** 01

**Status:** done（2026-09-13）

- [x] **错误分类**：`LlmErrorKind`（`providers/mod.rs`）按 `(HTTP 状态, 错误体特征)` 归类
      `auth`（401/403）、`model_not_found`（404 + model 相关体）、`network`（reqwest 层失败）、
      `context_window`（400 + 长度相关体）；**未知情形返回 None 保留原始错误串**，不误标
      （429 限流是可重试的瞬态，也不归类）
- [x] **用户可操作提示**：`Error::LlmClassified { kind, message, raw }`——`message` 为中文
      可操作文案（例：「provider 鉴权失败：请到「设置 · 模型与密钥」检查 api_key」），
      原始错误串进 `PendingContext.diagnostic`（`with_diagnostic`，不碰 `kind` 以免影响
      决策 130 的动作路由），面板渲染为次要信息行 `诊断：…`（PendingDossier）
      ——注意：agent 节点的重试耗尽包装原本会把分类压成 `Error::Validation`，
      已改为跨 attempt 保留 `last_classified` 并穿透（executor.rs `agent_node`）
- [x] **浏览器断言**：`frontend/e2e/provider-misconfig.spec.ts`（E2E-④）——坏 provider
      （恒 401 mock）→ 断言 `.msg` 是中文提示且**不混入**原始英文串 → 断言 `.ctx` 诊断行
      保留 401 + 原始体 → 断言「重试」「终止」按钮可用
- [x] **恢复路径真验**：`fixProvider()`（PATCH base_url 切回脚本 mock）→ 点「重试执行」→
      推进到 merge_approval
- [x] **配置页连通性验证**：`POST /providers/test`（**决策 160**，显式记录为决策 111/112
      的扩展）：对未保存的表单值发最小真实请求（max_tokens=1、15s 超时），失败按
      `LlmErrorKind` 归类；成功/失败都 200，结论在结果体；L3 契约用例 ×3
      （401 分类 / 掩码沿用于真请求 / 缺 key 400）；前端表单「测试连接」按钮 + 结果行
      （`buildProviderTest` 掩码省略规则与保存同源，vitest ×3）
- [x] **掩码语义**：探针响应体不含 api_key（契约用例断言原始密钥不出现在响应）；
      `***` 回显值不覆盖真值（`PATCH` 同规则）
- [x] 全量闸门绿 + playwright 四条全过（含本票新用例）

**实现期暴露的两个真实缺陷**（本票用例打红，另立票记录）：

- 票 11：合入后主仓库索引/工作区陈旧（决策 158）——由票 01/02 暴露；
- 票 12：设计类阶段 retry_exhausted「重试执行」死按钮（决策 159）——由本票恢复路径暴露。

**不在本票范围：** 让用户从界面上改 `.env` 或环境变量（v1 无此概念）；自动探测正确模型。
