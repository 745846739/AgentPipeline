# 05: 浏览器走 UI 创建 provider / 项目 / 任务

**What to build:** 主流程的前三步——配置模型与密钥、添加项目、新建任务——在端到端测试里
**全部被绕过**：`harness.ts:337-355` 直接 `POST /providers`、`POST /projects`、`POST /tasks`，
playwright 从看板开始接管。于是这三步的表单校验、提交后的跳转、失败提示、空状态引导
（`Board.svelte:57-60` 的「还没有项目…」、`SettingsProviders.svelte:213` 的「还没有 provider…」）
完全没有端到端证据。

而用户是**必须**从这三步开始的：没有 provider 时创建任务会被拒（决策 56），没有项目时
「新建任务」按钮根本不渲染（`TopBar.svelte:74`）。这三步任何一个坏掉，用户根本到不了现有
两条用例覆盖的中段。

**Blocked by:** 01

**Status:** done（2026-09-13）

- [x] **先失败一次再成功**：空 home 启动 → 打开界面 → 断言空状态引导可见（无 provider 文案、
      无项目文案）；这条同时验证「首次启动的第一印象」
- [x] 走 UI 建 provider（`SettingsProviders.svelte` + `ProviderForm.svelte`）：填 vendor / model /
      context_window / base_url / api_key → 提交 → 断言出现在列表且 **api_key 显示为掩码不回显明文**
      （决策 112，前端 `***` 规则）
- [x] 走 UI 建项目（`SettingsProjects.svelte` + `ProjectForm.svelte`）：选 fixture 仓库路径 → 提交 →
      断言项目出现。**覆盖校验失败路径**：填一个不存在的路径 → 断言界面给出明确错误
      （对应后端 `projects.rs:35-53` 的三条校验：非目录 / 非 git 仓库 / unborn HEAD，至少验前两条）
- [x] 走 UI 建任务（`NewTaskDialog.svelte`）：选项目、填标题与**描述**、评审模式选 agent →
      提交 → 断言跳转到 `#/task/{id}`（`NewTaskDialog.svelte:44` 的 `router.navigate`）
- [x] **断言描述真的进了 prompt**：任务描述经 `{task_description}` 进模板
      （`executor.rs:954`、`templates.rs:35`）。用 mock 侧记录收到的 user prompt，
      断言其中含所填描述——这是「用户填的字有没有被用上」的唯一端到端证据
- [x] 断言「依赖任务 ID（逗号分隔）」字段的解析（`NewTaskDialog.svelte:40-42`）：
      填两个 id → 断言后端建出两个依赖关系（或按 v1 语义断言当前行为，若语义有疑须在票面记录）
- [x] 更新 `harness.ts`：播种逻辑不再直接建 provider/项目/任务（保留为**可选**的快速路径，
      供其他用例复用，但本票新用例一律走 UI）
- [x] 全量闸门绿 + `just frontend-e2e` 全过

**注意：** 不得为了通过而放宽校验（如去掉路径存在性检查）。校验失败路径的断言是**验收项**，
不是障碍——用户填错路径得不到明确提示同样是主流程 bug。
