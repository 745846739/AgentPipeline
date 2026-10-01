# 打断策略原语上提（interrupt-policy）

**Status:** done（决策 355/356，票 01 已收口）

> **来源**：2026-09-30 架构体检 ⑤ 号卡 + 拷问定案。决策落 `docs/decisions.md` 355。
> 领域词条「打断策略（interrupt policy）」已补进 `docs/glossary.md`。

## Problem Statement

「一个事件只打扰人一次」在两处各实现一遍、两套互不相通的词汇：

- **watch 闸门**（`pipeline/foreman.rs` watch）：全局开关 → 人轮排队 → 失败退避
  （`WatchFailureState.waiting`）→ 去抖窗口 → 任务冷却 → 小时上限 + 上限通知去重
- **notify 礼貌**（`notify.rs`）：`notification_class` → `is_quiet_hours` →
  `NotifyPoliteness` / `resolve_politeness`

胶水散在 `scheduler/mod.rs`（pending_attention_kind）与 `storage/attention.rs`（wakes()）。
决策 350 那次触发面口径调整要在四个文件间推理。两个引擎内部都不浅——浅的是概念
没有自己的接口。

## Solution

共享原语（去抖窗口、按主体冷却、小时上限 + 上限通知去重）上提成 core 顶层纯模块
（暂名 `crates/core/src/interrupt.rs`）：无 I/O，时钟照旧从 `Store` 读（决策 64 的
唯一时钟源不动）。watch 与 notify 各当一个适配器。

## Constraints

- **词汇不统一**：attention vs politeness 各叫各的（拷问定案；决策 350 刚收口过，
  再动词汇等于重演）
- 不合并两引擎（推送与唤醒媒体不同）
- watch 的跨进程姿态不动（决策 210 / 226）

## 执行顺序

全盘点第五批。
