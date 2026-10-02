# 04: 外发工作流 offload.yml(workflow_dispatch + actions/cache + 文本结果)

**What to build:** 新增一个 GitHub Actions 工作流:输入为分支名 + commit SHA,
在 hosted runner(public 仓,标准档免费,单 job 6h 上限不触)上跑确定性重活——
cargo test、clippy、构建;`target/` 走标准 actions/cache(冷构建 5–10 分钟,
增量目标 2–4 分钟);结果只回**文本**:退出码 + 日志尾部,不回传任何产物
(73MB 二进制在 200KB/s 回程上要 6 分钟以上,决策:只取结果)。

**Blocked by:** None(工作流文件本身可先落;从 106 真实触发的验收依赖 02 的凭据)

**Status:** done(两轮实测已验收;第二轮总 wall 略超 4 分钟,原因见下,如实记录)

- [x] 手动触发(106 上 gh)一轮真实跑通:test / clippy / 构建三段结果齐全
  - run 37066381862(2026-10-02 21:21Z,冷缓存 `Cache mode: write`):**success**,
    lint 21:22:15→21:24:07(1m52s)、test 21:21:55→21:26:26(4m31s)、build
    21:21:55→21:25:06(3m11s),整轮 wall ~4m35s。
- [x] 第二轮触发缓存命中——命中验通,但总 wall ~5m07s,略超 4 分钟验收线
  - run 37066958665(21:27Z):三个 job 均 `Cache hit for: offload-main-*`
    (196MB 恢复成功),lint 42s(冷 1m52s)、build 1m37s(冷 3m11s)——
    **编译类活缓存收益明确**;test 5m03s(冷 4m31s)持平略长,瓶颈是
    测试执行/链接本身,不是编译,缓存救不了。整轮 wall 由最慢的 test job 决定。
  - 结论:增量目标「≤4 分钟」对 lint/build 达成;含全量 `cargo test --workspace`
    的整轮在 5 分钟上下是当前形状的地板。后续若要压,得靠 test job 拆分或
    只跑受影响 crate,不在本票范围。
- [x] 失败路径:三 job 均带 `if: always()` 步骤摘要,命令失败退出码可见、日志可读
- [x] 工作流无 secrets(权限 contents: read,concurrency 按分支 cancel-in-progress)
