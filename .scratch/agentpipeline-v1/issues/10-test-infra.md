# 10: 测试基建（四接缝与 testkit）

**What to build:** testing.md 要求的可测试性接缝与测试工具箱：Clock trait + ManualClock、AGENTPIPELINE_HOME + TestHome、进程组终止器 trait + RecordingKiller、手动 scheduler tick；testkit（FakeAgent 脚本驱动、git fixture 远程/冲突/symlink 陷阱、SseRecorder）；L2 三套件 + L3 契约 + 冒烟测试。

**Blocked by:** None（can start immediately）

**Status:** done（已实现；FakeAgent 驱动的 L4 属票 11/19）

- [x] 四接缝在生产代码落地并被 L2/L4 真实使用（决策 143）
- [x] testkit：FakeAgent/NodeScript、git_fixture（with_remote/conflict/symlink_trap/{test_command} 模板）、RecordingKiller、ManualClock、SseRecorder
- [x] L2：cursor_lifecycle 26 例 + git_chain 13 例 + scheduler_tick 18 例
- [x] L3：api_contract 35 例 + smoke 2 例
- [x] 缺口（归票 19）：fail_tool_n 形态、超长工具结果注入、慢滴流式步进
