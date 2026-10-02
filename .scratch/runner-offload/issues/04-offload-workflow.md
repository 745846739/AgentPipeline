# 04: 外发工作流 offload.yml(workflow_dispatch + actions/cache + 文本结果)

**What to build:** 新增一个 GitHub Actions 工作流:输入为分支名 + commit SHA,
在 hosted runner(public 仓,标准档免费,单 job 6h 上限不触)上跑确定性重活——
cargo test、clippy、构建;`target/` 走标准 actions/cache(冷构建 5–10 分钟,
增量目标 2–4 分钟);结果只回**文本**:退出码 + 日志尾部,不回传任何产物
(73MB 二进制在 200KB/s 回程上要 6 分钟以上,决策:只取结果)。

**Blocked by:** None(工作流文件本身可先落;从 106 真实触发的验收依赖 02 的凭据)

**Status:** ready-for-agent

- [ ] 手动触发(本机 gh 即可)一轮真实跑通:test / clippy / 构建三段结果齐全
- [ ] 第二轮触发缓存命中,增量耗时 ≤4 分钟
- [ ] 失败路径:命令失败时退出码与日志尾部仍可读,不是静默绿
- [ ] 工作流不使用任何 secrets(公仓原则:不把凭据暴露进 runner 日志)
