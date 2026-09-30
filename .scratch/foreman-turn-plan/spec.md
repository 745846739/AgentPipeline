# 值班长 TurnPlan：组装裁定抽成纯计算（foreman-turn-plan）

**Status:** ready-for-agent

> **来源**：2026-09-30 架构体检 ⑦ 号卡 + 拷问定案。决策落 `docs/decisions.md` 356。
> **依赖 foreman-split（决策 351）**：拆目录后有 foreman/ 才有自然的切分线。

## Problem Statement

`respond_inner` ~800 行：档位解析、简报构建、工具广告集、取史+trim+锚点注入、内联
压缩、超窗强制压缩的 LLM 循环、工具往返、ask 槽提取、归因解析、SSE、落库、预算判界
全在一个函数。纯件（trim_history / situation_drift / parse_attribution）各有单测，
但历史真 bug 全长在「怎么调」上——决策 288「23 次调用全成功仍被 30 分钟墙钟整段
砍掉」坏的是循环外界，而测试只有 FakeAgent 整轮脚本，粒度粗到只有碰巧钉住才红。

## Solution（窄边界，拷问定案）

**TurnPlan 只管组装裁定**：(会话、cfg、档位、转录、预算计数器快照) → LlmRequest +
工具广告集 + 压缩/截断/预算判定，落 `foreman/turn_plan.rs`；respond_inner 瘦成编排 +
SSE + 落库。重试裁定（输入 outcome 序列 → 下一动作的纯函数）作同票后半，不动
FakeAgent 接缝。

**明确不做**：宽边界（LLM 循环推进也抽进去）——与 SSE/落库交织，切开要动接缝
消费方式。

## 姿势基准

`model_request.rs::RequestPlan` 的「接口就是测面」头注（决策 249 片①）是同款模板。

## 执行顺序

全盘点第六批（最后）。票序：01 → 02。
