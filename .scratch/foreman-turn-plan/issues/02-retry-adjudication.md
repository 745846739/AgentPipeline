# 02: 重试裁定纯函数

**What to build:** 决策 356 后半——「重试裁定」抽成纯函数：输入 outcome 序列
（传输类 / 配置类 / 上下文超窗 / 连续超时轮数…）→ 下一动作（原样重试 / 空白重跑 /
挂起 / 收口）。合并现散在 respond_inner 循环里的各分支判据：决策 298（按错误类别
分流）、295（超窗压一次再试不按 agent_retry_max 盲试）、320（连续超时四段梯子）、
288/233（attempt 级重试与墙钟界）。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `foreman/turn_plan.rs`（或同 effort 旁挂文件）：`fn adjudicate(outcomes) -> Action`
      纯函数；循环各分支改查裁定
- [ ] 单测：四段梯子全路径（第 2/3 次不挂、第 4 次挂起）、传输类重试不追加错误 turn、
      配置类不进下一轮、超窗压缩一次重试、attempt 耗尽
- [ ] FakeAgent 接缝零改动（裁定不引入新替换点，决策 250 姿势）
- [ ] 验证：超时/重试族 e2e（含 e2e-14）照绿；core 全量 + lint 绿
