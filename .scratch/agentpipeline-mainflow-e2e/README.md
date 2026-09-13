# 主流程端到端可信度（执行计划）

来源：2026-09-13 针对「有没有 web 端到端用例」的核查。结论是：现有 `frontend/e2e/` 两条
playwright 用例覆盖了主流程**中段**，但入口（vite dev server）、产物形态（未加载真实内嵌产物）、
真实失败模式（闸门被短路、mock 只覆盖理想返回）三处与用户实际使用不对齐——存在
「测试全绿、用户一上手就卡」的窗口。本目录把这些缺口落成 10 张票，按优先级补齐。

验收口径：**用户从装好启动到看到合入结果的主流程上，每个不可避免的环节都有会真跑的端到端用例。**

## 依赖图

```
01 浏览器走真实产物（harness 改造，公共前置）─────────┐
├── 02 闸门真跑（真实工程 + 真测试命令）              │
├── 03 provider 配错可理解可恢复                     │
├── 05 UI 三步创建（provider/项目/任务）             │
├── 06 人工评审分支 + 合并「返回修改」                │
├── 07 日志/对话内容 + 刷新恢复                       │
├── 08 真进程重启恢复                                │
├── 09 并发第二任务                                  │
└── 10 纳入闸门 + 产物新鲜度守卫                     │
                                                     │
04 真模型全流程手动冒烟（独立，不依赖 01）────────────┘
```

01 是单点瓶颈（其余 8 票都由它解阻），因此**先做 01**，之后 02–10 并行可取。

## 优先级

**P0（先做，决定的不是覆盖率而是结论是否成立）：** 01 → 02 / 03 / 04
**P1（主流程入口与正常分支）：** 05 / 06 / 07
**P2（真实但低频，及工程面）：** 08 / 09 / 10

## 关键约束

- 每票验收落在既有闸门内：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`；前端加跑 vitest / svelte-check / build 与 `just frontend-e2e`。
- 与决策冲突必须显式标注编号（AGENTS.md）。已知需标注处：
  票 08 与决策 152（in-process 恢复）、票 10 与决策 147（闸门组成）、票 03 与决策 111/112（provider 面）。
- **替换边界不变**（决策 148 / 151）：只换 LLM 响应流，工具 / git / 命令 / 策略全部真跑。
- 只 Chromium（决策 144）；不得为过测试弱化决策 104 / 118 / 128 / 157 的安全语义。

## 完成状态

| 票 | 状态 | 登记 |
|---|---|---|
| 01 浏览器走真实产物 | done | harness 改走单二进制同源托管 + bundle 守卫；`assertEmbeddedBundle` 先验 |
| 02 闸门真跑 | done | fixture 变真 Node 工程；`gate.spec.ts`（E2E-③）断言系统命令真跑 + 失败分流 |
| 03 provider 配错可恢复 | done | 错误分类 + 中文提示 + 诊断保留；`provider-misconfig.spec.ts`（E2E-④）；「测试连接」端点（决策 160） |
| 04 真 LLM 全流程冒烟 | done | `llm_smoke.rs::real_llm_drives_full_flow_to_merge_approval`（`#[ignore]`）：真模型驱动完整主流程到 `merge_approval`，实测 570s / 794k tokens / 26 runs；途中自动应答 3 次 UserDecision。修了冒烟装置三处缺陷（goto 候选轮换、失败命令带输出、设计文档断言接受绝对路径） |
| 05 UI 三步创建 | done | `create-flow.spec.ts`（E2E-⑤×5）：空状态引导 / provider（含测试连接 + 掩码）/ 项目（坏路径报错）/ 任务跳转 / 描述进 prompt / 依赖字段；harness 增 `seedless` 与 `prompts()` |
| 06 人工评审 + 返回修改 | done | `review-branch.spec.ts`（E2E-⑥×3）：human 面板三件套 / 同端点反结论断言 / 打回附意见进流转原因 / 返回修改 → 二次推进 → done；harness 增 `reviewMode`、`backendLogs()`、多轮脚本；暴露缺陷票 13（决策 161） |
| 07 日志/对话 + 刷新恢复 | done | `logs-reload.spec.ts`（E2E-⑦×2）：命令内容/对话文本可断言（text 步骤置 submit 后）/ 无刷新实时推进 / 刷新恢复 pending 面板 / setOffline 断网容错 + 收敛 / 刷新后合入到 done |
| 08 真进程重启恢复 | done | `restart_recovery.rs`（Rust spawn，票面降级预案形态；不推翻决策 152，补其未覆盖的进程边界）：并行分支窗口 SIGKILL → 同 home 重启 → 归队续跑到 done，join 恰一次、无 worktree / 分支残留；暴露孤儿 running 挂起缺陷（决策 162）+ mock Submit 后收尾文本修正 |
| 09 并发第二任务 | done | `concurrent.spec.ts`（E2E-⑧×3）：双任务互不阻塞 + 看板多卡归位 + 双 pending 待办计数 / 并行双分支分组渲染 + resume 带对 `cursor_id`（决策 91）/ 基准前移后 approval 重置重走阶段 A（决策 96）。harness 增 `additionalTasks` 与按任务 id 路由（决策 165）。**暴露并修复 3 个缺陷**：卡片动作按钮被整卡链接覆盖（决策 164）、SQLite 快照升级失败 `BUSY_SNAPSHOT`、libgit2 建 worktree 的 TOCTOU（决策 163①②） |
| 10 纳入闸门 + 产物新鲜度 | done | `scripts/e2e-artifacts.sh`（两层新鲜度：前端源码 vs `dist`、cargo 增量重编）+ `just default` 聚合 lint/test/frontend/frontend-e2e + Makefile 镜像 `check-*`；无 CI 已显式记录；守卫经反向验证（决策 166，扩展 147） |
| 11 合入后工作区陈旧（缺陷） | done | `git.rs::sync_checked_out_worktree` + 决策 158 + 回归用例 |
| 12 设计类阶段重试死按钮（缺陷） | done | 动作表落点改 `entry_node` + 决策 159 + 动作失败横幅 |
| 13 合并「返回修改」后投影陈旧（缺陷） | done | 决策端点提交后立即 `sync_task_projection` + 决策 161 + 回归用例 |

每票完成后同步 `docs/testing.md` §9 / §10 的用例清单与闸门归属。

## 收口（2026-09-13）

全部 13 票 done。最终闸门：`cargo fmt --check` / `clippy --workspace --all-targets -- -D warnings` /
`cargo test --workspace` = **515 passed, 0 failed**（另有 2 个 `#[ignore]` 真 LLM 冒烟）；
前端 vitest **96 passed** + `svelte-check` 0 error / 0 warning + playwright **17 passed**
（`make check-e2e`，含产物新鲜度守卫）。

**验收口径达成**：用户从装好启动到看到合入结果的主流程上，每个不可避免的环节都有真跑的端到端用例。

**过程中暴露并修复的真实缺陷（共 8 个，串行测试与 mock 全绿时均不可见）**：票 01/02 → 合入后工作区陈旧
（决策 158）；票 03 → 设计类阶段重试死按钮（决策 159）；票 06 → 决策端点投影陈旧（决策 161）；
票 08 → 孤儿 `running` 任务重启后永久挂起（决策 162）+ mock `Submit` 后不收尾；票 09 → 看板卡动作按钮
被整卡链接覆盖（决策 164）+ SQLite `BUSY_SNAPSHOT`（决策 163①）+ libgit2 建 worktree TOCTOU（决策 163②）。
另有 1 处测试基建修正（决策 165：mock 附加任务按 id 路由）与 1 处闸门扩展（决策 166）。
