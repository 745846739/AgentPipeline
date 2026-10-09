# 01: 流水线闸门跑前端单测（develop / merge 闸门扩面）

**来源:** 任务 `01M4CD59Y977ZQ0GMY9MPSFFMX`（文案纪律扩面）两轮评审失败的根因分析。
第二轮打回的是 `frontend/src/lib/copy-discipline.test.ts` 的覆盖缺口，而这个文件所属的
前端单测套件**从不进流水线闸门**：项目 `test_framework = cargo` → `test_command_for`
只跑 `cargo test --quiet`，`Makefile:69` 的 `check-frontend`（1187 用例）不在闸门内，
评审节点又不跑测试。于是「前端全绿」只是 commit message 自报，**评审看的面与闸门跑的面
不重合**。106 实测（69–71s / 峰值 207MB / 1187 用例 / 服务不受影响）已排除可行性顾虑，
剩下的全是选型。成本与污染面见 `../spec.md`。

**What to build:** 先选型（A 新增 `unit_test_command` 列 / B 走 `test_framework` 的
raw 分支 / C Makefile `check-test` 扩面），再落地：

- [ ] 选型并写清理由（A = 30 文件最干净；B = 零表结构但污染 4 处 prompt 文案、动
      prompt cache 前缀；C = 语义最贴切但拖慢 check-test job）
- [ ] 落地所选形态：闸门命令在 `develop.validate_output` 与 merge 闸门两处都真跑前端单测
- [ ] 负例用例：改坏一条前端用例 → `develop.validate_output` 变红并带失败输出打回；
      恢复 → 绿（防「配置了没生效」）
- [ ] 选型理由落 `docs/decisions.md`；`docs/pipeline-spec.md` §validate_output 那行与
      `docs/operations.md:424` 同步
- [ ] 106 上启用前复测内存（同机带服务跑）

**Blocked by:** None（可立即开工；但**建议在任务 `01M4CD59Y977ZQ0GMY9MPSFFMX` merge 之后**，
避免与那单的评审面混在一起）

**Status:** ready-for-agent

**边界.** 只管单元测试面，不纳入 playwright e2e（决策 147）；不动 `test_command_for`
的框架映射表（决策 392）除非选型要求；不改 review 节点的静态评审定位。

## Comments

### 2026-10-09 · triage 裁决 → ready-for-agent，选型 C

**C · Makefile `check-test` 扩面（加 `cd frontend && npm test`）+ `test_framework = make check-test`。**
落点核实：`check-test` 现为裸 `cargo test --workspace`（Makefile:91），命令走 `Some(raw)` 既有
分支，零表结构。**排 B**：4 处 prompt 文案污染 + 动 prompt cache 前缀（380/381）+ `&&` 复合全仓
零先例——省一次 30 文件，换来每次改 prompt 的持续代价。**排 A**：30 文件只为一个与
`lint_command` 同构的列；C 同样零表结构且顺决策 168（Makefile 是闸门唯一权威，闸门跑「本仓
自己的 check」），`make check` 作 raw 值已有先例。**代价（接受）**：本地与 CI check-test 各慢约
70s、CI 中与 frontend job 重复跑 vitest。**红利**：闸门经 make 走工具链钉住（PATH /
RUSTUP_TOOLCHAIN），多一层 gate-failure-attribution 类事故的防护。
**开工约束**：`executor.rs` / `templates.rs` 与并行会话在飞改动重叠——独立 worktree 或等其
收口后再动；三票建议序 03 → 01 → 02。
