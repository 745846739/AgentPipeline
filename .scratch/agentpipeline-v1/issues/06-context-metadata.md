# 06: 上下文管理与元数据抽取库

**What to build:** 上下文管理 L0–L4 库（容量估算、软/硬限判定、工具结果修剪、压缩保留最近 N 轮、L4 动作规划）与结构化元数据抽取（submit_metadata 工具 schema 派生、三级降级解析、失败回填 retry_prompt）。

**Blocked by:** None（can start immediately）

**Status:** done（已实现，库层完成；运行时接线归票 11/16）

- [x] 软限 0.6 / 硬限 0.9 / keep_recent_rounds=5，规则表压缩摘要
- [x] trim_read_file / trim_run_command / trim_list_dir 已被工具执行器真实使用
- [x] L3 compact_messages 与 L4 plan_l4 完成但无运行时调用方（归票 11）
- [x] 三级降级解析（tool_call → 围栏 json → 末尾平衡 JSON），坏参 fail-fast（决策 33 机制层）
