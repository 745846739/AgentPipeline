# 04: 飞书报文分支——通用 webhook 直连不了飞书机器人，转换要进代码而不是外置进程

**Status:** done（2026-09-24）

**What to build:**（沿用 268 的触发面与礼貌策略，只动**最后拼 payload 那一步**）
`[notify]` 加 `format = "generic" | "feishu"`（缺省 `generic`，不破坏 268 契约）；
`notify.rs` 把 payload 构造收成**单一分流点** `payload_for(format, …)`（`attribution_body`
先出归因文本、再按 format 选 generic 六字段 / 飞书
`{"msg_type":"text","content":{"text": "title\nbody"}}`）。
飞书那端安全设置用**自定义关键词 `AgentPipeline`**（`title` 固定前缀 `[AgentPipeline]`
命中），**不做签名校验**（发送端不引签名依赖，要做也是后续独立 format 的事）。

**背景（2026-09-24 会话裁决）**：用户问「离线通知如何对接飞书」——通用 JSON 与飞书
机器人报文格式不兼容，直填机器人 URL 会被拒。曾议外置 relay 脚本，用户问「把转换器
写在代码中，成立一个新模块会不会更好」，裁决取**仓内格式化分支**（不取外置进程）：
外置 relay 多一跳、多一个丢通知的环节且宕机不在现有日志覆盖面内；仓内改动面只有
payload 构造几行，政策逻辑（cooldown/quiet/class 映射/归因白名单）一行不动。

**接缝事实**：
- `notify.rs::notify()` 的 payload 构造在 `should_notify` 通过、占坑之后（约 212-219 行）；
- `NotifyConfig`（`config.rs`）已有 `webhook_url/cooldown_sec/quiet_hours`，`deny_unknown_fields`；
- 跨语言 fixture `tests/fixtures/notification_policy.json` 钉的是**何时通知**的政策表，
  与报文体无关——本票**不碰** fixture 与前端；
- `serve.rs` 挂出口处把 `NotifyConfig` 传进 `WebhookNotifier::new`。

**明确不做**：不做签名（sign/HMAC）；不做飞书卡片/富文本（纯文本够用，有证据再议）；
不动政策语义与共享 fixture；不新造通知台账；不引任何新依赖。

**Blocked by:** None（决策 270 落档后实现）


- [x] 决策 270 落档
- [x] `format` 解析（缺省/显式/非法拒）3 条 + 两 payload 形状单测 2 条 + L2 飞书分支 1 条
- [x] `NotifyFormat` 枚举 + `payload_for` 分流（归因白名单分流前共用）+ `serve.rs` 透传
- [x] docs：268 落地注记 / operations.md / testing.md 用例目录
- [ ] 全量门禁 + 两轴 code-review + 提交
