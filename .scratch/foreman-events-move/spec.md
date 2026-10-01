# 事件发射搬家（foreman-events-move）

**Status:** done（决策 352，票 01 已落地）

> **来源**：2026-09-30 架构体检 ⑥ 号卡。决策落 `docs/decisions.md` 352（显式修订决策 249 的宿主名）。
> 全盘点最便宜的一张卡：纯文件搬家、半天量级，适合当热身。

## Problem Statement

决策 249 把请求组装与调用切成深模块，但 `emit_node_started` / `emit_tool_event` /
`finish_run_with_sse` 与阶段产出常量 `OUTPUT_*` 留在编排壳 executor.rs——model_invoke.rs
与 model_request.rs 都得回头 import 老大哥（回边）。从模型一条工具事件到浏览器要穿
tools → model_invoke → executor → sse → app/stream 五处。

## Solution

新小文件 `crates/core/src/pipeline/events.rs`：发射函数 + `OUTPUT_*` 常量搬入。
executor / model_invoke / model_request 平依赖它。决策 249 的意图「事件形状只有一个主人」
完整保留，违背的只是「主人叫 executor」的字面——修订已写入决策 352。

## Constraints

- 不并入 run_ledger.rs（run 台账与事件形状是两个概念，合住造新混居）
- 纯文件搬家、行为零变化、不加新接缝

## 执行顺序

全盘点第二批（在 foreman-split 之后、pipeline-tool-catalog 之前）。
