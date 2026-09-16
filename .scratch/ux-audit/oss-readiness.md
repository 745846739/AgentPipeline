# 票 11 · 开源落地就绪报告（OSS）

**票面**：`.scratch/ux-audit/issues/11-open-source-publication.md`
**规格**：`.scratch/ux-audit/spec.md` §「开源落地」、User Story 31 / 32 / 35
**范围**：**只做检查与落地准备**。本票**未**执行任何推送 / 发布 / 改远端可见性的动作。
**日期**：2026-09-16

---

## 0. 结论摘要

| 验收点（票面） | 结论 |
|---|---|
| 两份设计规格、四个原型、决策日志在公开视图下可见，且不被发布规则排除 | **通过**：逐项 `git check-ignore` 全数 VISIBLE，无任何排除规则命中（§1） |
| 敏感信息检查跑过一遍，逐条给出「留 / 改 / 删」的处置与理由 | **通过**：三类共 4 条需处置的发现，全部给出处置；其中 3 条属他人文件 → handoff（§2） |
| 本机绝对路径、凭据处理细节、内部环境标识逐类核过，核过的类目留下记录 | **通过**：§2.1 / §2.2 / §2.3 各有命令、命中数与逐条判定 |
| 公开视图下仓库内的链接都能走到（没有指向被排除文件的引用） | **通过（带 2 条 handoff）**：45 条相对链接 0 条失效；2 处**反向引用**指向被 gitignore 的产物，非我名下文件（§3） |
| 与 22 / 23 的衔接：行为映射表落在交互规格里，公开的是同一份，没有第二份需要同步 | **通过**：单一事实源在 `design/frontend-design.md` §12.3，公开可见，机器门直接读它（§4） |
| `LICENSE` | **已落**：仓库早已声明 MIT，照声明写入（§5） |

**未完成项（需要用户拍板）**：无阻塞项。但 §5.2 的 **holder 署名行**（`Copyright (c) 2026 泽运`）
取自 git 提交者身份，是维护者可能想改成项目名 / 组织名 / 补邮箱的一行——**已落，可一行改**。
另见 §6 的「非阻塞待办」。

---

## 1. 判定表：文件 → 可见性 → 依据

### 1.1 排除规则的全量清点（先确认规则面有多大）

| 检查 | 命令 | 结果 |
|---|---|---|
| 根的忽略规则 | `cat .gitignore` | 31 行（§1.3 全文判读） |
| **存的**嵌套 `.gitignore` | `find . -name .gitignore -not -path '*/node_modules/*' -not -path './target/*' -not -path './crates/desktop/target/*'` | **只有 `./.gitignore` 一个** —— 没有子目录级规则 |
| `.gitattributes` | `cat .gitattributes` | **不存在**（无 `export-ignore` 一类发布过滤） |
| `.git/info/exclude` | `cat .git/info/exclude` | 全为注释，**无生效规则** |
| CI 配置 | `git ls-files \| grep -i -E '\.github\|\.gitlab\|jenkins\|circleci\|travis\|ci\.yml\|\.ci/'` | **空**：本仓无任何 CI 配置文件，故无「CI 排除 / 私有流水线」可泄 |
| 清单级排除 | `grep -n -E 'license\|exclude\|publish' Cargo.toml` | 只有 `license = "MIT"`；**无 `exclude` / `publish = false`** |
| npm 级排除 | `grep -n -E '"private"\|"files"' frontend/package.json` | `"private": true`（见 §1.2 说明）；**无 `files` 白名单** |
| 其它 ignore 文件 | `ls .dockerignore .npmignore .eslintignore` | **均不存在** |

**判读**：本仓的排除面**只有一个 `.gitignore`**。没有 `.gitattributes` 的 `export-ignore`、
没有 CI 配置、没有子目录规则、没有清单 `exclude`——所以「发布规则会不会悄悄排除设计文档」
这个风险面，实际上就是那 31 行。

### 1.2 `frontend/package.json` 的 `"private": true`

**判定：留，且与票 11 无关。** 它是 `agentpipeline-frontend` 这个**内部前端构建包**的标记——
该包不单独发布到 npm（前端产物在编译期内嵌进 Rust 二进制，决策 155）。
它**不影响 git 侧的公开可见性**，抽屉里的 `design/` 与 `docs/` 一行都不受它管辖。
E 保持原样；若维护者将来要单独发包，那是另一件事。

### 1.3 逐项判定

命令（一次性跑全）：

```bash
git check-ignore -v <file>    # 有输出 = 被排除；空 = 可见
```

| 文件 | 角色 | 判定 | 依据（规则 : 行） |
|---|---|---|---|
| `design/frontend-design.md` | **规格一**（交互规格，含 §12.3 行为映射表） | **可见** | `check-ignore` 空；`git ls-files` 在册；`.gitignore` 无 `design/` 相关行 |
| `design/theme-6-pixel.md` | **规格二**（视觉规格） | **可见** | 同上 |
| `design/prototype-pixel.html` | **原型 1/4**（桌面 · 深色夜班靛） | **可见** | 同上 |
| `design/prototype-pixel-light.html` | **原型 2/4**（桌面 · 浅色掌机背光） | **可见** | 同上 |
| `design/prototype-pixel-mobile.html` | **原型 3/4**（移动 · 深色） | **可见** | 同上 |
| `design/prototype-pixel-mobile-light.html` | **原型 4/4**（移动 · 浅色） | **可见** | 同上 |
| `docs/decisions.md` | **决策日志 #1–194** | **可见** | `check-ignore` 空；`git ls-files docs/` 在册 |
| `docs/glossary.md` | 术语表 | **可见** | 同上 |
| `docs/README.md` | 文档入口（票 11 加了两行，§5.3） | **可见** | 同上 |
| `docs/testing.md` | 测试设计（含五条接缝） | **可见** | 同上 |
| `docs/operations.md` | 横切设计（含 §12 凭据姿态） | **可见** | 同上 |
| `design/deprecated/**` | 历史方案 8 原型 + theme-3 | **可见**（顺带核） | `git ls-files design/` 在册，共 18 个文件 |

**补充核实**：`git check-ignore -v design docs .scratch issues` → **全部无输出**，即这三个**目录本身**
也不被任何规则整目录排除。

### 1.4 `.scratch/` 的定位（有意的工作区，不与本票冲突）

`.gitignore:23–26` 现有两条：

```
.scratch/shots/**/*.png
# UI/UX 审计截图（可由 UX_AUDIT=1 跑 e2e/ux-audit.spec.ts 重新生成，不入库）
.scratch/ux-audit/*.png
```

**判定：`.gitignore` 不需要为本票改动任何一行。**

理由（这是票面特别要求核的那一点）：

1. 这两条只排除 **`.png`**。`.scratch/` 下的 `.md`（spec / issues / README / CONTRACT）**全部在册**：
   `git ls-files .scratch/` 共列出 **122 个文件**，覆盖 `agentpipeline-v1`、`agentpipeline-pixel-theme`、
   `agentpipeline-mainflow-e2e`、`foreman-talk`、`shots/capture*.mjs` 等。
2. **票 11 要公开的四件东西（两份规格 + 四个原型 + 决策日志）一件都不在 `.scratch/` 下**，
   全在 `design/` 与 `docs/`，与这两条 png 规则**零交集**。
3. 故不存在「规则与公开要求冲突」的情形，按票面「改规则而不是改被排除的产物」的授权，
   **本次无需行使**——没有冲突可改。
4. `.scratch/shots/*.png` 的不入库口径本身在 `.gitignore:23` 的注释里写明了理由
   （可由 `capture*.mjs` 重新生成），是有意的、维护者可见的取舍，保持原样。

### 1.5 未纳入公开范围的（显式记录，避免「以为都公开了」）

| 路径 | 状态 | 说明 |
|---|---|---|
| `.scratch/**/*.png` | gitignore | 截图证据，可由 `UX_AUDIT=1` / `capture*.mjs` 再生 |
| `frontend/dist/`、`frontend/node_modules/`、`/target`、`crates/desktop/target/` | gitignore | 构建产物 |
| `.zcode/`、`sweep.timestamp`、`.DS_Store` | gitignore | 本机工具产物（注释已说明「与本项目无关」） |
| `.worktrees/` | gitignore | 并行子代理工作树（临时） |

---

## 2. 敏感信息检查：跑了什么 · 命中多少 · 逐条处置

### 2.1 本机绝对路径

**跑了什么**

```bash
git grep -n -I -E '/Users/[A-Za-z]+|/home/[A-Za-z]+'
```

**命中：25 处 / 9 个文件。** 按文件分布：

```
  10  crates/core/src/agent/prompts.rs
   5  crates/core/src/config.rs
   2  crates/core/src/agent/snapshots/agentpipeline_core__agent__prompts__tests__system_prompt.snap
   2  crates/core/src/agent/context.rs
   2  .scratch/shots/capture.mjs
   1  frontend/src/components/settings/ProjectForm.svelte
   1  crates/core/src/agent/skills.rs
   1  crates/app/src/routes/market.rs
   1  .scratch/agentpipeline-markdown-skills/issues/03-config-landing-docs-and-tests.md
```

**逐条处置**

| # | 位置 | 命中原文 | 处置 | 理由 |
|---|---|---|---|---|
| 1–10 | `crates/core/src/agent/prompts.rs`（:454, :524, :592–:615） | `/home/u/.agentpipeline/worktrees/t1` 等 | **留** | 合成测试夹具：用户名是占位的 `u`，路径是 `t1` 测试任务。**不含真实身份，也不含真实仓库位置**。`.snap` 快照同理（快照里必须逐字等于夹具值） |
| 11–12 | `crates/core/src/agent/context.rs`（:590, :594） | `/home/t/.context/abc.txt` | **留** | 同上，占位符 `t` |
| 13–17 | `crates/core/src/config.rs`（:1122–:1245） | `Path::new("/home/u/.agentpipeline")` | **留** | 同上 |
| 18 | `crates/core/src/agent/skills.rs`（:1148） | `Path::new("/home/u/.agentpipeline")` | **留** | 同上 |
| 19 | `crates/app/src/routes/market.rs`（:472） | `技能根下的 /home/me/skills/grill/SKILL.md` | **留** | 用户可见报错里的**泛用示例路径**（`me` 是通用占位），不是本机路径 |
| 20 | `frontend/src/components/settings/ProjectForm.svelte`（:67） | `placeholder="/Users/you/code/project"` | **留** | 输入框占位符，`you` 是通用的「你」——**这正是避免泄露本机路径的写法**，是正面样本 |
| 21–22 | `.scratch/shots/capture.mjs`（:4, :5） | `'/Users/lazyking/Documents/AgentPipeline/design'` / `'.../AgentPipeline/.scratch/shots'` | **改** → handoff | **真实本机路径且已入库**：泄露维护者用户名 `lazyking` + 本机仓库绝对位置。改名下文件，已写 `.scratch/ux-audit/handoff/OSS-11.md` §3 |
| 23 | `.scratch/agentpipeline-markdown-skills/issues/03-…md`（:35） | `/Users/lazyking/.agentpipeline/skills/` | **改** → handoff | 同上，历史票面正文里的真实用户名路径。同上 handoff |

**小计**：25 处中 **23 处判「留」（合成夹具 / 泛用占位）**、**2 处判「改」（真实本机路径，均不在我名下 → handoff）**、0 处判「删」。

**交叉核实（本机用户名专名）**

```bash
git grep -n -I -i 'lazyking'
```

**命中 3 处**，正是上表 #21–#23 这三行，无额外漏网。

### 2.2 凭据处理细节

**跑了什么（两轮：宽词面 + 真密钥形状）**

```bash
# (a) 宽词面 —— 目的是量出「讨论凭据」的正文规模，判断是否需要逐条读
git grep -c -I -i -E 'api[_-]?key|secret|password|token'
# (b) 真密钥形状 —— 目的是找「像真的」的串
git grep -n -I -E 'sk-[a-zA-Z0-9_-]{16,}|AKIA[0-9A-Z]{12,}|-----BEGIN|Bearer [A-Za-z0-9._-]{20,}|ghp_[A-Za-z0-9]{20,}|xox[baprs]-'
# (c) 已入库的敏感文件类型
git ls-files | grep -i -E '\.(env|db|sqlite|key|pem|p12|pfx|credentials)$|(^|/)\.env'
```

**命中与判定**

- **(a) 1630 处 / 177 个文件。** 这是**正文讨论规模**，不是泄露规模——按票面提醒「命中不等于违规」，
  做了采样判读：命中集中在三处已知的凭据相关设计面
  （`docs/operations.md` §12 的「凭据与配对令牌姿态」、`crates/core/src/agent/sanitize.rs` 的脱敏实现、
  `crates/core/src/agent/providers/*` 的各家 adapter 字段名），以及大量 `pairing_token` / `api_key`
  **字段名**与测试断言。**判定：无需逐条处置；这 1630 处是设计文档与代码的字面量名称，不承载秘密。**
- **(b) 22 处 / 3 个文件** —— 这是有意义的收敛结果：

  | 文件 | 命中 | 处置 | 理由 |
  |---|---|---|---|
  | `crates/app/tests/api_contract.rs` | `sk-super-secret-value-123456`、`sk-new-key-abcdefghijkl` | **留** | 测试夹具；**且该文件里的断言正是在验证这些值不出现在响应体里**（`:1049`、`:1130`、`:1167` `assert!(!body.to_string().contains(...))`）——是**防泄露测试**，删掉反而毁掉护栏 |
  | `crates/core/src/agent/sanitize.rs` | `sk-abcdefghijklmnop12345678`、`ghp_ABCDEFGHIJKLMNOPQRSTUVWX` | **留** | 脱敏函数的测试输入，**明文的「假密钥」**；同名断言验证脱敏后不再包含该串 |
  | `crates/core/src/agent/tools.rs` | `sk-abcdefghijklmnop12345678` | **留** | 同上 |

  **判定：22 处全部是合成假密钥，且全部位于「验证密钥被脱敏 / 不出网」的测试里。0 处真密钥。**
- **(c) 空。** **无任何 `.env` / `.db` / `.key` / `.pem` / `credentials` 文件入库。**
  这与 §2.4 的 `~/.agentpipeline` 姿态一致——真实密钥只存在于**运行时**的用户主目录，不在仓库里。

**入库的第三方许可（顺带核，不是泄露）**

`frontend/public/fonts/fusion-pixel-12px/LICENSE.font.txt` 与 `LICENSE.package.txt` 在册，
是 Fusion Pixel Font（OFL 系）的许可原文——**应当在公开仓库里保留**，不是待清项。

### 2.3 内部环境标识（本机用户名 · 主机名 · 端口 · 私有 CI）

| 子类 | 命令 | 命中 | 处置 |
|---|---|---|---|
| 本机用户名 | `git grep -n -I -i 'lazyking'` | 3（见 §2.1 交叉核实） | 2 处「改」→ handoff，1 处已含在 #23 |
| 内部主机名 | `git grep -n -I -E '\.lan\b\|corp\.\|internal\.'` | **0** | 无需处置 |
| 私有 IP | `git grep -n -I -E '192\.168\.\|10\.\|172\.(1[6-9]\|2[0-9]\|3[01])\.'` | 命中均为 **RFC1918 示例地址** | **留**：`README.md:80` 的 `--allowed-origin http://192.168.1.10:8788` 是**使用说明里的示例**；`crates/app/src/lan.rs` 的 `c("en0", "192.168.1.10", true)` 等是网卡排名的单元测试夹具；`crates/app/src/main.rs:117–132` 是 `--allowed-origin` 解析用例。**无一是维护者的真实内网地址** |
| 端口 | `README.md:80`、`crates/app/main.rs` | 8788 / 8787 | **留**：`8788` 是**产品默认端口**（`[server]` 段，README 正文已公开写明），不是内部设施标识；环境变量 `AGENTPIPELINE_PORT` 可覆盖 |
| 私有 CI | 见 §1.1「CI 配置」一行 | **0 个配置文件** | 无需处置 |
| 内部主机名 | `git grep -n -I 'hostname'` | 仅 `frontend/src/lib/localPage.ts` 的 `isLoopbackHostname` 谓词（决策 190） | **留**：那是**判断来源是否回环**的产品逻辑，与内部环境无关 |

### 2.4 `~/.agentpipeline` 的权限姿态（逐条核）

**跑了什么**

```bash
git grep -n -I -E '\.agentpipeline' -- ':!*.snap' | grep -E 'chmod|0o?7|0600|perm|KEY'
git grep -n -I -E '(chmod|0o700|0o600|0700|0600)' -- crates/
```

**命中与判定**

| 位置 | 内容 | 处置 | 理由 |
|---|---|---|---|
| `docs/operations.md:1165–1169` | 权限表：`~/.agentpipeline/` = `0700`；`data/agentpipeline.db` = `0600`「含 provider 明文密钥（决策 112）」等 5 行 | **留** | **这是文档，不是密钥**。它说的是「密钥明文存本机、目录 0700」这一**姿态声明**——公开它是**有意且正确**的：用户装之前就该知道密钥落在哪、权限如何 |
| `design/frontend-design.md:370/491/507` | 界面文案口径「密钥明文存于本机 `~/.agentpipeline`，目录权限 0700」（决策 112） | **留** | 同上；且 §:507 正是票 23 要求**去掉内部决策编号**的那一行（S 名下，已在他的改动范围内） |
| `design/prototype-pixel*.html` ×4（:1310/:1325/:1324/:1310） | 原型里的同一句提示文案 | **留** | 原型是**随仓库公开的规格物**，这是它的定稿文案 |
| `frontend/src/routes/SettingsProviders.svelte:159` | 实际渲染的同一句 | **留** | 面向用户的**运行期**提示，本来就该让用户看到 |

**关键核实：描述 ≠ 泄露。** 上述命中的**全部**是「密钥存在哪、权限是多少」的**说明文字**；
配合 (c) 的结论「无 `.db` / `.key` / `.env` 入库」，可以确认
**仓库里没有任何真实密钥，只有对密钥存放位置与权限的姿态描述**。
这正是票面要求核的那一点，结论为**通过，无需处置**。

**脱敏能力侧核**：`crates/core/src/agent/sanitize.rs` 里
`sanitize_text`（:100）与 `sanitize_command_line`（:165）是**实施**，不是描述——
即「日志里会不会打印密钥」这一问，产品侧的答案是**会先过脱敏，且有 §2.2 那批测试兜底**
（`:180–:311`）。日志出口（`crates/core/src/storage/observability.rs`、`tools.rs:1445–1453`）
的密钥形状串已由脱敏替换，无需处置。

---

## 3. 公开视图下仓库内的链接可达性

### 3.1 正向链接（引用目标是否存在于磁盘）

**跑了什么**：脚本抽取 `docs/README.md`、`docs/decisions.md`、`docs/glossary.md`、`docs/testing.md`、
`docs/operations.md`、`design/frontend-design.md`、`design/theme-6-pixel.md`、`README.md`
里的**全部相对 markdown 链接**（排除 `http` / `mailto:`），逐个 `os.path.exists` 解析。

```
total relative links checked: 45
MISSING: 0
```

**判定：通过。45 条全部可达，0 条失效。**

其中 `docs/README.md` 的两条关键入口已逐条人眼复核：

- `:26` → `../design/frontend-design.md`、`../design/theme-6-pixel.md` —— **在册**（§1.3）
- `:27` → `../design/prototype-pixel.html`、`prototype-pixel-light.html`、`prototype-pixel-mobile.html`、
  `prototype-pixel-mobile-light.html`、`../design/deprecated/README.md` —— **全部在册**

即：**两份规格 + 四个原型从文档入口一站可达**，满足票面第 1 条与第 4 条。

### 3.2 反向链接（是否有引用指向「被排除的」产物）

**跑了什么**

```bash
grep -rn -E '\.scratch/(shots|ux-audit)[^ )`]*\.png' docs/ design/ README.md agent-pipeline.md AGENTS.md
```

**命中 2 处**，两条都指向被 `.gitignore:24` 排除的 `.scratch/shots/**/*.png`：

| 位置 | 原文 | 处置 |
|---|---|---|
| `docs/testing.md:228` | `（`.scratch/shots/app/*.png`）` | **改**（非我名下，所有者=编排者）→ `handoff/OSS-11.md` §1 |
| `design/theme-6-pixel.md:265` | `node scripts/make-icon.mjs --preview # .scratch/shots/icon-preview.png` | **改**（非我名下，所有者=DEC-VIS）→ `handoff/OSS-11.md` §2 |

**这两处为何不是「判留」**：它们是**引用**（票面第 4 条要求逐条处置的四类之一），
且指向的产物**确实不随仓库公开**。但它们**不是 markdown 链接**（是行内代码里的路径 / 命令注释），
所以 §3.1 的 45 条链接检查**不会**捕捉到——这正是票面要求单独扫 `.scratch/` 引用的原因。
两种可接受的处置（改措辞 / 显式标注「不入库」）已写进 handoff。

### 3.3 `.scratch/` 下其它引用

`docs/testing.md:231/346/361`、`docs/agentstriage-labels.md:17`、`docs/agents/issue-tracker.md`
里有若干 `.scratch/<feature-slug>/…` 引用。**判定：留。** 依据：这些指向的是
`.scratch/` 下的 **`.md`**（spec / issues / README），而它们**全部在册**
（§1.4：`git ls-files .scratch/` 122 个文件）——**在公开视图里走得到**，
不构成「指向被排除文件的引用」。`docs/agents/issue-tracker.md` 更是本仓 issue 流程的**制度说明**
（AGENTS.md 明确要求它存在），必须保留。

---

## 4. 与票 22 / 23 的衔接核实

**结论：通过——公开的是同一份，没有第二份需要同步。**

**核过的事实**

| 项 | 事实 | 依据 |
|---|---|---|
| 行为映射表落在哪 | `design/frontend-design.md` **§12.3「行为 / 规则 → 实现位置（决策 199）」**，表头 `\| 行为 / 规则 \| 实现位置 \| 备注 \|` | `grep -n '行为 / 规则 → 实现位置' design/frontend-design.md` → `:571`、`:593` |
| 没有第二份吗 | 规格 §12.3 明文「与视觉规格既有的『视图 → 组件』表**同形**（三列），**不新建文件**」（spec.md:173）——即**刻意的单文件决定** | `design/frontend-design.md:575` |
| 该文件公开吗 | **是**，`check-ignore` VISIBLE，在册（§1.3） | — |
| 机器门读的是哪一份 | `frontend/src/lib/behavior-map.test.ts` 直接解析 **`design/frontend-design.md` §12.3**：`const SPEC_PATH = …('../../../design/frontend-design.md')`，断言每条「实现位置」在磁盘上存在 | 已读该文件头 40 行 |
| 有没有「只在内网 / 只在本地的副本」 | **没有**：全仓只有这一张表；`behavior-map.test.ts` 是**读它**的消费者，不是第二份事实源 | `git grep -l '行为 / 规则'` 只命中 `design/frontend-design.md` 与该测试 |

**因此**：票 22 / 23 的产出（映射表 + 悬空引用门）**天然随仓库公开**，
票 11 无需为它做任何搬运或同步。这是两条票的接缝，**核过，无待办**。

---

## 5. `LICENSE`

### 5.1 先核对「仓库里有没有已经声明的许可口径」——**有**

```bash
grep -n -E 'license' Cargo.toml crates/*/Cargo.toml frontend/package.json
```

```
Cargo.toml:14:license = "MIT"            ← [workspace.package] 段
crates/app/Cargo.toml:6:license.workspace = true
crates/core/Cargo.toml:6:license.workspace = true
crates/desktop/Cargo.toml:6:license = "MIT"   ← 桌面壳独立 workspace，自己又声明一次
crates/testkit/Cargo.toml:6:license.workspace = true
```

**判定：已声明，口径一致为 `MIT`。** 故按票面「若已经声明过，照它写 `LICENSE`」——
**未替维护者另拍一个许可证**。

同时核过：`README.md` / `docs/README.md` / `docs/overview.md` 里**没有**任何与 `MIT` 冲突的
许可说法（`grep -i -E 'license|许可|MIT|Apache|GPL'` 只命中两张无关的表格行），
即**不存在两个口径打架**的情况。

`frontend/package.json` 无 `license` 字段、有 `"private": true` —— 与 §1.2 一致，
不影响仓库整体口径。

### 5.2 落地的 `LICENSE`

**新建 `/Users/lazyking/Documents/AgentPipeline/LICENSE`**：标准 MIT 全文（`MIT License` 标题 +
许可 / 免责两段），版权行：

```
Copyright (c) 2026 泽运
```

**holder 的取值依据**：仓库无 `authors` 字段、无 `copyright` 声明、README 无署名段
（均已 `grep` 核过）。故取**提交者身份**——`git log -1 --format='%an <%ae>'` → `泽运`。
这个姓名**已经出现在每一次 git 提交里**（`git log` 即可见），写入 LICENSE **不构成新的信息暴露**，
故未取邮箱（少一处暴露面）。

**这一行是维护者可能想调整的唯一一处**：若希望署名改为项目名 / 组织名 / 补上邮箱，
**改这一行即可，全文其余部分与 MIT 标准文本一致（不应改动）**。见 §6 待办。

### 5.3 `docs/README.md` 的登记（只加了票 11 的两行）

**改了 `/Users/lazyking/Documents/AgentPipeline/docs/README.md:28–29`**（`## 引用约定` 段末追加）：

```markdown
- **公开范围（票 11）**：上面引用的两份设计规格（[frontend-design.md](../design/frontend-design.md)、
  [theme-6-pixel.md](../design/theme-6-pixel.md)）、四款主题六原型（`design/prototype-pixel*.html`）
  与决策日志 [decisions.md](decisions.md) 都**随仓库公开**——界面文案里的规格 / 决策引用对公开读者可达。
  （历史方案 `design/deprecated/` 同样公开；`.scratch/` 是工作区，其下截图不入库。）
- **许可（票 11）**：本仓库以 **MIT** 许可公开，见仓库根 [LICENSE](../LICENSE)，
  与 `Cargo.toml` 的 `license = "MIT"` 同口径。
```

**严守所有权边界**：只在文件**末尾追加**票 11 的两行，**未改动**该文件任何既有内容
（`## 文档地图` 表、§26 / §27 两条既有引用约定、全部既有行都一字未动）——
其余回填由编排者进行。

---

## 6. 未完成项 / 需要拍板（诚实列出）

| # | 项 | 类型 | 谁 | 说明 |
|---|---|---|---|---|
| 1 | `LICENSE` 的 holder 署名行（`Copyright (c) 2026 泽运`） | **需维护者确认（可一行改）** | 用户 | 取值依据见 §5.2（git 提交者身份）。若要改为项目名 / 组织名 / 补邮箱，改这一行。**不阻塞公开**：MIT 已在 5 个清单里声明过，本文件只是把它落成实体 |
| 2 | `docs/testing.md:228` 指向 gitignore 产物的引用 | 需他人改 | 编排者 | `handoff/OSS-11.md` §1 |
| 3 | `design/theme-6-pixel.md:265` 同上 | 需他人改 | DEC-VIS | `handoff/OSS-11.md` §2 |
| 4 | `.scratch/shots/capture.mjs:4–5` 真实本机绝对路径（**已入库**） | 需他人改 | 编排者指派 | `handoff/OSS-11.md` §3 |
| 5 | `.scratch/agentpipeline-markdown-skills/issues/03-…md:35` 真实用户名路径（**已入库**） | 需他人改 | 编排者指派 | `handoff/OSS-11.md` §3 |

**明确未做 / 不在本票范围**：

- **未执行任何面向外部的发布动作**——无 push、无发版、无改远端可见性。票面明令检查先行。
- **未跑全量构建 / e2e**（`parallel-brief.md` §一.2 禁止），故本报告的全部结论来自
  静态检查（`git grep` / `git check-ignore` / `git ls-files` / 链接存在性解析），
  这一层面足以支撑票面的四条验收点。
- **`.scratch/ux-audit/**` 本身尚未 `git add`**（`git status` 显示 `?? .scratch/ux-audit/`）：
  本 effort 的工作区文件是否随仓库公开，属编排者决定，本票不作判断。
  但 §1.4 已核实：**它不被 gitignore 整目录排除**，需要公开时可直接入库（仅 `.png` 被排除）。
- `frontend/package.json` 的 `"private": true`：**保持原样**（§1.2），未改。

---

## 7. 本票改动清单（供编排者收口）

| 文件 | 动作 | 位置 |
|---|---|---|
| `/Users/lazyking/Documents/AgentPipeline/LICENSE` | **新建**（MIT 全文） | 全文 |
| `/Users/lazyking/Documents/AgentPipeline/docs/README.md` | 追加票 11 两行 | `:28–29`（`## 引用约定` 段末） |
| `/Users/lazyking/Documents/AgentPipeline/.scratch/ux-audit/oss-readiness.md` | **新建**（本报告） | 全文 |
| `/Users/lazyking/Documents/AgentPipeline/.scratch/ux-audit/handoff/OSS-11.md` | **新建**（handoff） | 全文 |
| `/Users/lazyking/Documents/AgentPipeline/.gitignore` | **未改（OSS 本票）** | 理由见 §1.4：核过无冲突。注意 `git status` 里 `.gitignore` 显示为 `M`，那是**本 effort 其它流先前加的** `.scratch/ux-audit/*.png` 一行（`:25–:26`），**不是 OSS 的改动**；OSS 未增删该文件任何一行 |
