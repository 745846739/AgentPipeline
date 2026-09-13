# 04: 真模型全流程手动冒烟（`#[ignore]`）

**What to build:** 把真模型冒烟从「单节点」扩到**一条完整主流程**，让发版前有人能确认
「真 key 之下主流程走得通」。

**Blocked by:** None

**Status:** done

- [x] 扩展/新增 `#[ignore]` 冒烟：真 key + 真模型驱动**完整主流程**到 merge 阶段 A
- [x] fixture 用**真实可构建的小工程**（与票 02 同源），使闸门在真模型下也真跑
- [x] 断言关键接缝：每个节点都有 run 行、`submit_metadata` 在真模型返回格式下被正确解析、
      token 计量 > 0、无节点落到 `retry_exhausted`
- [x] 失败时输出**足够定位的诊断**（哪个 `(stage, node)` 的什么错误）
- [x] 在 `docs/testing.md` §3.2 更新为「单节点 + 全流程」两条，写明运行命令
- [x] **不进入任何自动门**（决策 142）；`#[ignore]` 保留
- [x] 票面记录**实测结果**

## 实测结果（成功的一轮完整运行）

| 项 | 值 |
|---|---|
| 环境 | 本机 macOS；provider 为本地 OpenAI 兼容代理 `http://127.0.0.1:8787/v1` |
| 模型 | `deepseek-flash`（vendor = openai 适配器） |
| 命令 | `AGENTPIPELINE_SMOKE_{VENDOR,MODEL,API_KEY,BASE_URL}=... cargo test -p agentpipeline-core --test llm_smoke real_llm_drives_full_flow_to_merge_approval -- --ignored --nocapture` |
| 耗时 | **570s**（约 9.5 分钟） |
| 结论 | **通过**：`tokens=794236 calls=16 runs=26`，停在 `pending(merge_approval)` |
| 人工介入点 | 自动应答 **3 次** `UserDecision`：review 的 `skip`（强制通过评审）→ test 的 `goto 修改测试用例` → test 的 `goto 修改业务代码` |

断言全绿：12 个 agent 节点各有 run 行、`design_doc` 阶段产出登记且文件真实存在、
`total_tokens / total_calls > 0`、命令记录里 `npm test` 退出码 0（真模型写的代码真过了测试）、
无 `retry_exhausted`。

## 本轮暴露的问题（测试自身，非产品缺陷）

首轮（未修前）**卡在 `gate_recheck` 出不来**：连续 8 次自动应答全打在「修改测试用例」
（`goto test.execute`），任务在 `test.validate_output` 的 `gate_recheck` 上死循环到上限。
根因是**自动应答策略**，不是流水线：

1. **固定取第一个 goto 候选** → `gate_recheck` 同时给「修改测试用例」（同阶段）与
   「修改业务代码」（develop），固定取前者时，若真实问题在业务代码就永远修不好。
   修复：候选按 `(stage, node)` 稳定排序后**按轮换序号取模**，两条出路各得机会。
   实测第三轮轮换到「修改业务代码」后流程即收敛到 merge_approval——**轮换是这次通过的关键**。
2. **诊断缺失败输出** → 命令记录只打 `exit code`，闸门失败的真实原因（断言文本、测试输出）
   全丢。修复：失败命令附 stdout / stderr 尾部，并打印带 `gate_failure` / `blocker` 的
   阶段产出元数据。
3. **设计文档断言过窄** → 只查任务目录下的相对路径。真模型可能登记**绝对路径**，
   断言因此误报。修复：绝对路径直接查、相对路径在两个根（任务目录 / worktree）里查，
   找不到时列出两个根的实际文件清单再报错。

这三处都是**冒烟装置自身**的缺陷（票面「暴露出的真实缺陷（若有，按缺陷修复后重跑）」
要求的正是把它们修掉重跑），产品流水线本身未暴露缺陷。

## 已知边界（票面要求写明，不得静默）

真模型输出不确定，本冒烟是**人工确认手段**而非回归门——它绿不保证明天绿。它的价值是把
「真模型路径完全无人知晓」变成「发版前有人跑过」。`#[ignore]` 保留，不进 `just test` /
`just frontend-e2e`（决策 142）。

> **与决策 142 的关系：** 本票不改变「外部不确定性不做回归门」的裁决，只是把同一规则下
> 的手动手段从单节点扩到全流程；运行记录落在本票面，不落任何 CI。
