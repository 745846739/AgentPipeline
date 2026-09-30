# 01: 打断策略原语收敛 interrupt.rs

**What to build:** 决策 355——新文件 `crates/core/src/interrupt.rs`（纯函数、无 I/O）：
去抖窗口、按主体冷却、小时上限 + 上限通知去重三件原语。`foreman::watch` 的
`WatchFailureState` 退避与冷却段、`notify::resolve_politeness` 的同型判断改为调用原语；
两套词汇与对外行为零变化。头注写明「一次事件一次打扰」的唯一实现在此。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] `interrupt.rs`：三件原语 + 纯函数单测（窗口边界、冷却到期、上限触发 + 上限通知
      只发一次）
- [ ] `foreman::watch` 去抖 / 冷却 / 小时上限段改调原语；`WatchFailureState` 的落库
      形状不动
- [ ] `notify.rs` 的 politeness 判断中同型部分改调原语；`notification_class` /
      quiet hours 判定留原处（那是 notify 自己的词汇）
- [ ] 验证：原语单测 + watch / notify 既有用例照绿；core 全量 + lint 绿
- [ ] 复演确认：决策 350 类触发面口径调整此后只碰 `interrupt.rs`（在头注里写明）
