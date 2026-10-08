# 把前端单测纳入流水线闸门

## 背景（事实，2026-10-08 实测）

流水线的 develop / merge 闸门 = `lint_command` + `test_command_for(project.test_framework)`
（`crates/core/src/pipeline/executor.rs::run_code_gate`）。本仓注册为项目时
`test_framework = cargo`，于是闸门只跑 `cargo test --quiet`（+ clippy），
**`Makefile:69` 的 `check` 里那套 `check-frontend`（vitest 1187 用例 / 87 文件）从不入闸门**。

后果是实证过的：任务 `01M4CD59Y977ZQ0GMY9MPSFFMX`（文案纪律扩面）第二轮评审打回的
是 `frontend/src/lib/copy-discipline.test.ts` 里的覆盖缺口——而该套件的「绿」只是
commit message 自报，评审节点又「不执行测试运行」（纯静态读码）。**评审判面与闸门判面
不是同一张面**，两侧的「绿」互相证明不了对方。

106 实测（`cd /opt/AgentPipeline/frontend && /usr/bin/time -v npx vitest run`，
跑前可用 1518MB / 2GB 机器）：

| 指标 | 值 |
| --- | --- |
| 墙钟 | 69–71s（vitest Duration 68.68s） |
| 峰值 RSS | 207 MB |
| 用例 | 1187 passed / 87 文件，0 failed |
| 服务 | `agent-pipeline` 全程 active，跑后内存完全回落、swap 未增长 |

→ 耗时与内存上**放进闸门可行**。剩下的全是选型问题。

## 三个候选形态（选型留到开工，本 spec 只列成本）

**A · 新增可选列 `unit_test_command`**（与 `lint_command` 同构，决策 139）
实测成本 **30 个文件**：迁移 `0045_*.sql` + `PROJECT_COLUMNS` / INSERT / `update_project`
+ `Project` 是无 `Default` 的全字段结构体（14 处 `Project { … }` 字面量编译必红）+
`update_project` 是位置参数签名（8 处集成夹具连带红）+ POST/PATCH 路由 +
project_analysis 探测清单 + 三处前端 + `git.rs` 探测映射。最干净，最贵。

**B · 把命令塞进 `test_framework`**（`test_command_for` 的 `Some(raw) => raw.into()` 分支）
零表结构改动，且**已有先例**：`Some("make check")`、`Some("sh <绝对路径>/gate.sh")`
（`crates/core/tests/integration/executor.rs:3699-3704`，直接拿来当闸门命令）、
`Some("npm test --silent")`（`llm_smoke.rs:191`）。代价是**污染四处文案**：
`templates.rs:65`（test.execute 的 user prompt 会打出「测试框架：`cargo test && npm test`」，
同一句话两种口径）、`templates.rs:314`（system prompt 把 shell 串塞进「使用目标项目的
测试框架（…）」）、`prompts.rs:26-34`（AGENTS.md 兜底系统段——**动它等于动 prompt cache
前缀**，决策 380/381 关注面）、`tools.rs:2268` + `analysis.ts:78-81`（值班长 JSON 与
核对清单读数）。另外 `&&` 复合命令**全仓零先例**（空格命令 5 处）。

**C · Makefile `check-test` 扩面 + `test_framework = make check-test`**
零表结构、零新列，闸门语义「跑本仓自己的 check」最贴切。代价：把闸门扩面藏进
`Makefile`（决策 168 让它成为闸门唯一权威，属于顺着它走）；但会拖慢本地 `make check-test`
与 CI 的 check-test job（四个 job 里最贵的那个）。

## 验收（无论选哪一形态）

- [ ] 闸门真跑前端单测：前端用例失败必须让 `develop.validate_output` 变红并打回 execute
- [ ] 负例钉住：临时改坏一条前端用例 → 闸门红；恢复 → 绿（防止「配置了但没生效」）
- [ ] 选型理由落 `docs/decisions.md`（接决策 406 之后的编号）
- [ ] `docs/pipeline-spec.md` §validate_output 那行与 `docs/operations.md:424` 同步更新

## 边界

- 不把前端 **e2e**（playwright）纳入——决策 147 明确排除 e2e，本票只管单元测试面。
- 不动 `test_command_for` 的框架映射表本身，除非选型要求（决策 392 明确「不动
  `test_command_for` 的映射」，选 A/C 都不违反，选 B 是走它的既有 raw 分支）。
- 106 上真正启用前**复测一次内存**（本次是只读探查，闸门是在服务同机跑的）。
- 不顺带改 review 节点「不执行测试运行」的定位——那是另一件事（见
  `.scratch/review-round-ledger/`），本票只让**闸门**这一侧有牙齿。
