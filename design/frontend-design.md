# 前端设计规格（v1 看板视图）

> 本文扩展 §12.11 前端交互设计，落地为可实现的前端规格。遵循既有决策：16（Svelte + TS + Vite）、
> 79（v1 只发看板）、76（单 SSE 通道）、153（传输层 Tauri 防御约束）、84（并行分支消歧）、49/69/101（allowed_actions 纯渲染）、
> 92（列归属与焦点游标）、34（stalled / archived 表示）、65（v1 只做应用内通知）。
> 视觉方向如有调整，以本文 + `design/prototype.html` 为准。

---

## 1. 定位

**一句话：** 一张挂在开发者本机的调度台——你看得到每列车（任务）在线路（流水线）上的位置，
看得到它正在说什么（流式会话）、跑了什么（命令日志）、花了多少（token），以及最要紧的：
**哪里停下来了等你拿主意（pending）**。

| 维度 | 结论 |
|---|---|
| 用户 | 在自己机器上跑 agent 流水线的开发者 |
| 首要工作 | 一眼看清"跑到哪、哪里等人"，并在 pending 时做决策 |
| 信息密度 | 高：流水线状态 + 流式输出 + 命令日志 + 成本同屏 |
| v1 范围 | 看板视图 + 任务详情 + 设置（项目 / provider）+ 全局指标（决策 79） |

---

## 2. 设计概念：夜间调度台

流水线的领域语言本身就是铁路语言：**线（track）、站（stage）、车（游标上的任务）、
岔口（sync-check 汇合）、折返线（review→develop / merge→test 打回）、信号灯（状态色）**。
视觉系统全部由此展开，不做第二套装饰语言。

四条原则：

1. **轨道即导航。** DAG 不是配图，是骨架。看板列头挂在轨道站点上，卡片内置迷你轨道线，
   任务详情页复用同一轨道组件作 hero。用户在任何页面看到的都是同一条线。
2. **状态即信号灯。** 五色信号系统（执行 / 等人 / 失败 / 完成 / 支线）只承载状态语义，
   不做装饰用色。全站无渐变、无投影堆叠。
3. **等人优先。** pending 是唯一被允许"响"的东西：琥珀色、顶栏待办计数、任务详情右侧
   dossier 栏。其余一切保持安静。
4. **等宽只给可执行物。** 命令、路径、diff、工具参数、ID、token 数字用等宽字体；
   界面文案用中文短句，按钮写"合入 / 返回修改 / 补充说明并继续"，不写"提交"。

---

## 3. 视觉语言（Design Tokens）

### 3.1 色彩

| Token | 值 | 语义 |
|---|---|---|
| `--ink-900` | `#0C121B` | 页面底色，港湾蓝黑 |
| `--ink-800` | `#111A26` | 面板 / 卡片 |
| `--ink-700` | `#16212F` | 悬浮 / 二级面板 |
| `--line` | `#223246` | 描边、分隔线 |
| `--line-soft` | `#1B2838` | 弱分隔、轨道暗段 |
| `--text-hi` | `#E9EFF7` | 主文本 |
| `--text-2` | `#9DB0C5` | 次文本 |
| `--text-3` | `#64788F` | 弱文本、占位 |
| `--signal-go` | `#3FD68F` | running / 通过 / 游标亮段 |
| `--signal-caution` | `#F2B33D` | pending / stalled（唯一告警色） |
| `--signal-stop` | `#E5544B` | failed / 打回 / 破坏性动作 |
| `--signal-done` | `#7C93AC` | done（刻意退后，不抢视线） |
| `--branch-dev` | `#4CC3E0` | develop-design 支线 |
| `--branch-test` | `#A78BFA` | test-design 支线 |
| `--diff-add` / bg | `#7EE2B0` / `#16342A` | diff 增行 |
| `--diff-del` / bg | `#F0918A` / `#3A2320` | diff 删行 |

**规则：** 一张卡片上最多出现一种信号色。queued / waiting 用 `--text-3` 呈灰。
对比度：正文对底色 ≥ 7:1，弱文本 ≥ 4.5:1，信号色只作点睛不承载正文。

### 3.2 字体

| 角色 | 字体 | 用法 |
|---|---|---|
| 拉丁 UI / 数字 / wordmark | Barlow（400/500/600） | tabular-nums 用于一切数字列 |
| 密集表头 / 列头 / 卡片 meta | Barlow Semi Condensed（500/600） | 站点标签、成本行 |
| 中文正文 | PingFang SC（系统栈，不加载 CJK webfont） | 13–14px 基准，行高 1.55 |
| 可执行物 | JetBrains Mono（400/500） | 命令、路径、diff、工具参数、ID、token 数 |

字阶：11 / 12 / 13（基准）/ 14 / 16 / 20 / 26。纯本地工具，字体经 CDN 引入并带完整系统回退栈。

### 3.3 形状与密度

- 圆角：面板与卡片 6px，药丸 / 标签 3px。全局只有这两档。
- 无卡片投影。层级靠底色（900 → 800 → 700）与 1px 描边表达。
- 看板列宽 280px，列间距 12px，横向滚动；详情内容区最宽 980px 左对齐。
- 间距基准 4px 栅格。

### 3.4 动效

唯一主动效：**首次加载轨道自绘**（stroke-dashoffset 600ms，游标点随后淡入）。
其余全部是响应状态的动效：游标点沿轨滑动（stage_changed，280ms ease-out）、
pending 琥珀呼吸（2.4s 周期）、流式输出尾随光标。`prefers-reduced-motion` 全部关闭。
按钮长耗时异步（§12.11）：点击即 loading 禁用，SSE 回执后复位。

---

## 4. 信息架构与路由

```
/                    看板（按 project 过滤；决策 58：前端按 project_id 过滤看板）
/task/:id            任务详情（轨道 hero + 时间线/会话/命令/产出/Diff）
/settings/projects   项目管理（创建即 POST /projects + 可选 /projects/analyze）
/settings/providers  模型与密钥（决策 112：明文存储，读接口回显 ***）
/metrics             全局指标（GET /metrics）
```

- 无 SvelteKit，Vite + Svelte 5（runes）+ 轻量 hash 路由（本地应用，无 SEO 诉求）。
- **传输层 Tauri 防御（决策 153）：** ① 本前端是**纯 API 客户端**，一切数据经 HTTP + SSE，不假设部署形态（桌面化 = Tauri 只当外壳，不走 IPC 重写）；② SSE 消费用 **fetch 流式读取**（可携带自定义头），不用 `EventSource`——它带不了自定义头，跨源过不了决策 128 防护；③ 所有写请求**恒携带** `X-AgentPipeline` 头（决策 128 旁路，桌面 webview origin 靠它放行）；④ API base 收敛**单一配置点**：默认同源相对路径，留注入覆盖口（桌面壳注入 `http://127.0.0.1:{port}`）。
- 顶栏常驻：wordmark ｜ 项目切换 ｜ 状态过滤（全部 / 执行中 / **待处理 N** / 等依赖 / 排队 / 已完成 / 已结束（失败·取消））｜ 新建任务。
- **待处理计数**（`has_pending_cursor` 的任务数，决策 92）是顶栏唯一的动态计数入口，
  点击下拉列出全部 pending 任务（琥珀点 + 阻塞原因摘要），点击进入对应任务。

---

## 5. 看板视图

### 5.1 布局

```
┌────────────────────────────────────────────────────────────────────────────┐
│ AgentPipeline [项目▾] 全部 执行中 待处理③ 等依赖 排队 已完成 已结束 [+ 新建任务] │
├────────────────────────────────────────────────────────────────────────────┤
│   ●init───●architect──┌●design∥┌──●develop───●review───●test───●merge──●done│  ← 轨道脊线（列头即站点）
│                       │ dev ∥ test │┄┄ 折返虚线（review→develop 等）┄┄┄┄ │
├────────────────────────────────────────────────────────────────────────────┤
│ [init]   [architect-design]   [develop-design ∥ test-design]   [develop] …  │
│  卡片      卡片                卡片（双药丸）                   卡片         │
└────────────────────────────────────────────────────────────────────────────┘
```

- **列 = 阶段**（决策 92 的"独立槽位"）：8 列 = init / architect-design /
  **develop-design ∥ test-design（双轨合并列）** / develop / review / test / merge / done。
- **sync-check 全站不展示**（决策 107 + 2026-09-11 评审确认）：它不占游标行、任务永远不会
  "停在"它上面——不设列、轨道不设站、卡片迷你轨不设刻度、时间线不显示其 join 记录。
  汇合由双轨直接合流进 develop 表达；`waiting_join` 由分支药丸的"等待汇合"字形承载；
  backtrack 呈现为回到 architect-design 的自动流转记录。
- 列头即轨道站点：站点圆点渲染该列任务聚合状态（有 pending → 琥珀；在跑 → 绿），
  轨道把各列连成一条线；并行区间画成真实分岔双轨（青 / 紫），在 develop 站前直接合流
  （不设汇合站点）；打回路径（sync-check→architect、review→develop、merge→test / develop）画为
  轨道下方的灰色虚线折返弧，出现打回任务时短暂点亮为红。
- 终态任务：done 列展示已合入任务；failed / cancelled 卡片停留在其终态游标所在的列，
  用「已结束（失败·取消）」过滤桶收敛（决策 131）；archived 默认不可见
  （`include_archived=false`，决策 101）。

### 5.2 任务卡片解剖

```
┌──────────────────────────────────────┐
│ 实现用户登录                     12m  │   标题（text-hi）＋ 持续时长（等宽数字）
│ ─●────●────●──○──○──○──○──○─         │   迷你轨道线：9 站点（去 sync-check），游标点亮
│ develop.execute · 第 2 次尝试         │   游标药丸（并行区间双药丸，决策 84）
│ 45.2k tok   214 calls  kanban/t-0042 │   成本行（等宽，token 计数实时累加；金额 v2——差距④）
└──────────────────────────────────────┘
```

- **迷你轨道线是卡片的身份特征**，替代进度条：9 个刻度点串成一条线（10 阶段去掉
  sync-check），完成的点亮，游标处是滑动圆点，并行区间用青紫双色刻度。
- 游标药丸：`分支 · 节点`（mono）。pending 的药丸按**分支色**着色（develop 青 /
  test-design 紫）并加 ⏸ 字形表达阻塞，可分别展开（决策 84：按分支着色，动作集按游标
  独立下发）。
- pending 卡片：琥珀左缘 + 阻塞原因一行 + 动作按钮组（`allowed_actions` 纯渲染，
  `kind: resume` 走 `POST /resume`，`side_effect` 走专用端点，决策 69/105）；
  `requires_input` 的动作渲染行内输入框（仅 `info_insufficient` 有自由输入，决策 79）。
- stalled 卡片（`stalled = true`，决策 34）：整卡琥珀描边 + 「已滞留 3 天」角标。
- **禁止拖动**（§12.11）。卡片整卡可点进详情。

### 5.3 空态

空列不放插画，一句话：如 done 列空 →「还没有任务走到终点。跑完一个任务它会出现。」
整板空 →「新建第一个任务，流水线会从 init 开始走。」

---

## 6. 任务详情页

### 6.1 结构

```
┌───────────────────────────────────────────────────────────┬───────────────┐
│ ← 实现用户登录        running · 12m34s · 45.2k tok · kanban/t-0042        │
│ ┌──────────────── 轨道 hero（全 DAG，节点级 ✓/●/○/⏸/✗/↩）───────────────┐ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│ [时间线] [会话] [命令与输出] [产出文件] [Diff]                             │
│                                                                          │
│  （当前 tab 内容）                                                        │  │ 待办
│                                                                          │  │ dossier
│                                                                          │  │ (pending
│                                                                          │  │  时出现)
└──────────────────────────────────────────────────────────────────────────┴───────────────┘
```

- **轨道 hero**：§12.4.2 流水线视图的实现体（9 站，不含 sync-check——决策 107，它不是
  任务位置），节点状态图例沿用 ✓/●/○/⏸/✗/↩；并行区间双轨分岔；当前游标有滑动圆点与心跳微光。
- **待办 dossier（右栏 360px，仅 pending 时出现）**：G4 的展开实现。2026-09-11 确认以
  **持久面板**替代 §12.7 原定的弹窗形态——面板随任务常驻、不遮挡内容区，看板侧以
  toast + 顶栏待办计数提醒。内容：
  阻塞原因（`pending_reason.message`）、按分支分组的动作按钮（含行内输入框）、
  「agent 为什么这么判断」= 触发节点的会话直达链接（§12.4.3 联动）、
  `merge_approval` 时内嵌 Diff 面板、产出文件入口。任务不再 pending 时该栏收起，
  内容区回到全宽。

### 6.2 五个页签

| 页签 | 内容 | 数据源 |
|---|---|---|
| 时间线 | `kanban_transitions` 渲染的流转记录，并行区间两条交错记录以 `∥` 分支徽标区分；trigger 用词表（normal/retry/kickback/user_resume…）原样展示；sync-check 的 join run 记录不进入时间线（决策 107），backtrack 呈现为进入 architect-design 的自动流转行 | `GET /tasks/{id}/flow` + SSE |
| 会话 | 按 stage/node/attempt 分组的会话列表 → 会话查看器（§12.4.3 ASCII 的实现体）：System Prompt 折叠块、消息气泡、tool 调用卡（工具名 + 参数摘要 + 结果行数）、submit_metadata 元数据卡；子代理（`agent_type` ≠ main）以缩进子会话呈现；失败 attempt 以红色分隔条标注原因；**进行中的 run 实时流式渲染** | `/conversations` + SSE 流 |
| 命令与输出 | 命令行表（`✓/✗ 时刻 source 命令 耗时 exit`），行展开见首尾预览与完整输出（卸载文件走 `/commands/{id}/output`）；`source = agent | system` 用徽标区分但共用一表（§12.4.4） | `/commands` + SSE |
| 产出文件 | design.md / dev-plan.md / test-scenarios.md / review-report.md / review-diff.diff / test-report.md，Markdown 渲染 | `GET /tasks/{id}/files/{path}` |
| Diff | merge proposal：DiffStats 摘要（文件数 / 增删行 / 逐文件明细）+ unified diff 渲染 + 审批动作（合入 / 返回修改，决策 23：无"拒绝"）；`base_commit` 过期时后端会重置 approval，前端在 diff 顶部提示「基准已前移，diff 重新生成中」 | stage_outputs + `/files/merge-proposal.diff` |

- 人工评审（`review_mode = human`）：pending(human_review) 的 dossier 呈现三件套——
  变更 diff（`review-diff.diff`，系统生成，决策 124）、agent 预审报告、单元测试结果，
  底部「通过 / 打回并附意见」（`POST /review`）。

---

## 7. 设置与全局指标

| 页面 | 要点 |
|---|---|
| 项目 | 本地路径为唯一事实来源（决策 29）；创建后引导触发 `POST /projects/analyze`（伪阶段探测事实清单 + agent 摘要，决策 78），结果以核对清单呈现供确认；删除有活跃任务时禁用并说明原因（决策 105） |
| 模型与密钥 | provider 行 = (vendor, model, context_window)（决策 111）；`api_key` 输入框写后即掩码回显 `***`（决策 112），旁边固定一行提示「密钥明文存于本机 `~/.agentpipeline`，目录权限 0700」；`supported_adapters` 之外的行降级灰显 + 告警，不崩（决策 103） |
| 全局指标 | GET /metrics：成功率、各阶段平均耗时 / 重试率 / validate 通过率、token 消耗。全部以**轨道分段条形图**呈现（横条挂在轨道站点下），延续"轨道即导航"；无 KPI 卡片横排 |

---

## 8. 组件架构

```
frontend/
├── src/
│   ├── routes/            # Board / TaskDetail / SettingsProjects / SettingsProviders / Metrics
│   ├── api/               # 类型化客户端；TS 类型与 §4 数据模型一一对应（文档已是 TS interface）
│   ├── realtime/
│   │   ├── connection.ts  # EventSource 生命周期、退避重连、visibilitychange 处理
│   │   └── reduce.ts      # SSE 事件 → store 归约（按 branch 分拣，决策 84）
│   ├── stores/            # board.svelte.ts / taskDetail.svelte.ts / notifications.svelte.ts
│   └── components/
│       ├── pipeline/      # PipelineRail（轨道，3 种变奏：脊线 / 卡片迷你轨 / hero）、CursorDot、BranchPill
│       ├── board/         # BoardColumn、TaskCard、PendingActions、StalledBadge、NewTaskDialog
│       ├── task/          # TimelineView、ConversationViewer、CommandLog、FileViewer、DiffReviewPanel、ReviewForm
│       ├── render/        # ★ 公共渲染件（§12.11 复用表）：MarkdownView、MessageBubble、
│       │                  #   ToolCallCard、MetadataCard、CodeHighlight、DiffView、TokenMeter
│       └── settings/      # ProjectForm（含 analyze 清单）、ProviderForm
```

- `render/` 即 §12.11"外壳独立、渲染件复用"的落点：v1 会话查看器与 v2 对话窗口共用。
- `PipelineRail` 是全站唯一"重"组件：一份 DAG 拓扑数据（来自 petgraph 的静态形状，构建期内联）
  + 游标数组 → 三种密度变奏（脊线 120px / 迷你轨 16px / hero 200px）。
- token 计数组件 `TokenMeter` 唯一允许每秒多次重渲（流式累加），并做 `requestAnimationFrame` 合帧。

---

## 9. 状态与实时

### 9.1 数据流

```
GET /tasks?project_id=        → board 初始装载（决策 101，含分支级摘要）
GET /tasks/{id}/stream (SSE)  → 执行中 / pending 任务的实时更新（每任务一条，决策 76）
10s 对齐 tick 的轻量 refetch   → 兜底 waiting / queued 状态迁移（对齐 tick_interval_sec）
GET /tasks/{id}               → 详情页装载 + 断线重连后的全量校准（决策 76 唯一状态入口）
```

- board 打开时仅为 `status ∈ {running, pending}` 的任务开 SSE（数量受 `max_concurrent_tasks`
  约束，≤ 10 条连接）；其余状态靠 refetch。连接指数退避重连，`document.visibilitychange`
  恢复时立即校准一次。
- SSE 事件 → 归约表：

| 事件（§12.7） | board 归约 | 详情归约 |
|---|---|---|
| `node_started` / `node_finished` / `cursor_changed` / `stage_changed` | 列归属 / 站点信号色 / 迷你轨游标 | hero 状态 + 时间线追加 |
| `command_started` / `command_output` / `command_finished` | — | 命令表增行 / 追加输出 / 收尾 |
| `pending` / `pending_updated` | 顶栏计数 + 琥珀药丸（按 branch） | 打开 dossier，覆盖上下文 |
| `task_done` / `task_failed` / `task_cancelled` | 移列 + toast（按 NotificationPolicy） | 终态横幅 |
| `conversation_delta` / `tool_event`（决策 123） | — | 会话页追加消息 / 工具卡 / token 累加 |

- **通知策略实现**（决策 65）：toast 只对 pending / done / failed 弹（cancelled 不弹），
  同类 5 分钟 cooldown（`cooldown_sec`），22–8 免打扰（pending 豁免）；stalled 高亮走 SSE。

### 9.2 前端约束清单（来自主文档，实现时逐条对照）

- `allowed_actions` 纯渲染，每个 side_effect 必须有配对端点（决策 101/105）——前端不做动作白名单。
- 多游标 resume 必须带 `cursor_id`，省略仅在恰有一条游标时允许（决策 91）——按钮提交时从所属游标药丸取 id。
- pending 面板动作 = `continue / skip / goto`（resume 类）＋ 旁路动作（取消 / 拆分 / 换模型——决策 132 已把无端点的「放弃合入」「合并任务」移出动作集），
  两类按钮视觉分组，旁路动作弱化（决策 69/70）。完整映射见主文档 §5 的 **allowed_actions 权威总表**（决策 130）。
- 长耗时按钮异步 + loading 禁用（§12.11）；看板卡片禁拖（§12.11）。

---

## 10. 与后端契约的差距（需在实现前排掉）

| # | 差距 | 建议 |
|---|---|---|
| ① | SSE 事件表（§12.7）没有**会话流式输出**的事件类型，但 §12.11 要求 agent 文本 / 工具调用实时流式 | 在 `/tasks/{id}/stream` 增补 `conversation_delta`（`run_id` / `agent_type` / `branch` / `role` / `text`）与 `tool_event` 两类事件，落库仍走现有会话 API。**已采纳（决策 123）**：§12.7 已增补，本表差距①排掉 |
| ② | 看板只有一个 per-task SSE；board 的多任务实时性只能靠"活跃任务逐条开流 + tick 对齐 refetch"凑 | v1 照此实现（够用）；v1.1 可加 `GET /stream?project_id=` 全局通道作为升级路径，`reduce.ts` 归约表不变。**g6 确认照此实现** |
| ③ | 人工评审需展示"变更 diff"，但 `merge-proposal.diff` 在 merge 阶段才生成 | review 阶段由系统生成一份 develop 产出 diff（或复用命令表里的 `git diff` 记录），经 `/files/{path}` 下发。**已采纳（决策 124）**：系统生成 `review-diff.diff`，本表差距③排掉 |
| ④ | 成本以 token 计；"¥"换算需要 provider 价目 | v1 只显示 token 与调用次数（原型已同步为 token-only）；价目进 providers 表后再开金额。**g6 确认**：主文档 §12.4.2 的 ¥ mock 一并移除（决策 131） |
| ⑤ | 流式 token 计数事件未定义 | 随 ① 的 `conversation_delta` 带 `prompt_tokens` / `completion_tokens` 增量。**已采纳（决策 123）** |

---

## 11. 实施顺序

1. **骨架**：token 体系 + `PipelineRail`（三变奏）+ 路由 + API client（类型从 §4 直拷）。
2. **看板**：`GET /tasks` 装载 + 卡片 + 列语义 + 过滤 + 新建任务对话框；无 SSE 也能用（refetch）。
3. **任务详情**：hero 轨道 + 时间线 + 会话查看器（`render/` 组件就位）+ 命令表 + 文件。
4. **实时**：SSE 连接层 + 归约表 + 异步按钮 + 通知策略；pending dossier 与待办计数。
5. **Diff 审批与人工评审** → 6. **设置页与全局指标** → 7. 打磨：空态、reduced-motion、键盘可达、
   断线重连横幅。
