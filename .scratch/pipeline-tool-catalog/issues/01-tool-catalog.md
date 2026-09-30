# 01: agent/catalog.rs 规格表 + 空壳广告退场

**What to build:** 决策 353——新文件 `crates/core/src/agent/catalog.rs`：8 个内置工具
（write_file / edit_file / read_file / delete_file / list_dir / run_command /
submit_metadata / Skill）的 name + description + JSON-Schema parameters 一张表。
`model_request::tool_defs` 改从目录表取（空壳 `description: String::new()` +
`{"type":"object"}` 退场）；tools.rs 的 25 臂 dispatch 名字引用目录表。冻结断言钉
「目录名字集 = `BUILTIN_TOOLS` ∧ `ENV_TOOLS` ∪ `ENV_WRITE_TOOLS` ∪ `SERVICE_WRITE_TOOLS`
的分层对应」。

**Blocked by:** None

**Status:** done

- [x] `agent/catalog.rs` 一张表：name + description + parameters（参数 schema 照
      `execute()` 的解析逐字段对齐，含 edit_file 的 old_text/new_text、run_command 的
      workdir 等全部必填/可选）
- [x] `model_request::tool_defs` 从目录表生成广告集；工具顺序保持现役不变（provider 侧
      顺序敏感处先核）
- [x] 冻结断言：目录名字集与三份层名单 + `BUILTIN_TOOLS` 的对应；断言写进 tools.rs 或
      catalog.rs 的 tests
- [x] 抽查断言：每个工具广告出的 parameters schema 与 `execute()` 实际解析的字段一致
- [x] prompts.rs 里与参数形状重复的散文段收敛（描述以目录表为准，散文留纪律不留形状）
- [x] 验证：core 全量 + lint 绿；foreman e2e 与 executor e2e 照绿

## 注记（落地口径，2026-09-30）

1. **「25 臂 dispatch 名字引用目录表」的口径**：决策 353 的目录表只收 8 个内置工具，
   故 dispatch 只有 8 个内置臂改引 `catalog::*` 常量；`repair` / `spawn_sub_agent` /
   台账只读 / D 层各臂不在目录范围（名字主人分别是层名单与 foreman 清单），仍是字面量。
2. **`SUB_AGENT_TOOLS` 的两臂一并退场空壳**（票面只点名 model_request）：子代理的
   `tool_defs` 是同款空壳，且 `SUB_AGENT_TOOLS` ⊆ 目录名——按决策 353 标题
   「空壳广告退场」一并接目录表（code-review Spec 轴判为温和越权，回写票面）。
3. 票①的「run_command 的 **workdir**」：`execute()` 实际解析的参数名是 **`cwd`**
   （tools.rs `run_command`），目录表按 `cwd` 广告——票面措辞过时，按实现对齐。
4. checkbox ⑤ 核查结论：prompts.rs **没有参数形状散文**（`BASELINE_PREAMBLE` /
   `FORMAT_RULES` 只纪律性提及工具名），无可收敛项——「散文留纪律不留形状」本来就成立。
5. `SUBMIT_METADATA` 目录行的 description/parameters 是登记性占位：schema 随节点种类由
   结构体派生（决策 38 与校验同源），`def_for` 对它恒返 `None`（用例钉住）；
   该行载重的是名字与广告语——「8 个工具一张表」的形状选择。
6. 冻结断言口径：决策原文「目录名字集 = 三份名单并集」字面不成立（并集含 repair /
   task 等非内置名）；实现钉「目录名字集 = `BUILTIN_TOOLS` ∧ 与三份名单的分层对应」
   （与票面措辞一致），已随落地注记回写决策。
7. 验证：core lib 663（+7：catalog 6 + model_request 1）/ core 集成 463（4 ignored）/
   e2e 40 / app 224 / serve 84 + 5，fmt + clippy `-D warnings` 全绿。
