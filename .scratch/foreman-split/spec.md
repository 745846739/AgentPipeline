# 值班长 foreman.rs 拆目录（foreman-split）

**Status:** ready-for-agent

> **来源**：2026-09-30 架构体检 ① 号卡 + 拷问定案。决策落 `docs/decisions.md` 351。
> 全盘点首刀：最热文件（300 提交内改 46 次、4148 行）的最便宜深化——纯文件搬家，行为零变化。

## Problem Statement

`crates/core/src/pipeline/foreman.rs` 一个文件装八个互不隶属的概念：轮次存活注册表、工具目录、历史压缩、态势简报、人格文本、运行器（含 ~800 行 `respond_inner`）、归因解析、~700 行内联测试。外部调用方 `crates/app/src/routes/foreman.rs` 要 import ≥8 件散装物——值班长的接口不是「一个 ForemanRunner」。每条值班长特性都要过这堵承重墙，没有局部性。

## Solution

原地转成 `pipeline/foreman/` 目录，六个文件 + mod.rs re-export：

| 文件 | 装什么 |
|---|---|
| `registry.rs` | 三个静态表 + 全部 Drop 凭据 + 自由函数；**附注：捆成具名 `TurnRegistry`** |
| `catalog.rs` | `ForemanToolSpec` + `FOREMAN_TOOL_SPECS` + `foreman_available_tools[_except]` + `foreman_tooling` |
| `briefing.rs` | `ForemanBriefing` + `build_briefing` + `situation_fingerprint` / `situation_drift` |
| `conversation.rs` | 历史压缩与预算（`CompactionCache` / `summarize_input` / `trim_history` / 相关常量） |
| `runner.rs` | `ForemanRunner`（say / respond / respond_inner / watch / record_interrupted_turn / notify_reply_completed），目标 ~1300 行 |
| `attribution.rs` | `AttributionKind` + `parse_attribution` |

## Constraints

- 接口面一字不动：mod.rs re-export 保持外部 import 路径不变
- 行为零变化、不加任何新接缝（决策 250 的假 seam 教训）
- ~700 行内联测试跟随各自模块
- `FOREMAN_TOOL_SPECS` 本体与冻结断言不动（决策 247）

## 执行顺序

全盘点第一批（351 → 352 → 353 → 354 → 355 → 356）之首；`foreman-turn-plan` 被它阻塞。
