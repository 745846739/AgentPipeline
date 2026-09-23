# 02: 回环判定收成 host_policy module（Rust 侧）

**What to build:** 「什么算本机」只有一处实现：新建 `crates/core/src/host_policy.rs`，interface 只有一个 `is_loopback(raw: &str) -> bool`（归一是实现细节，不进 interface）；三个 Rust 消费者——出口策略（`egress.rs`）、技能来源仓的明文 http 放行（`repo.rs`）、服务绑定地址与配对令牌判定（`app` 的 `peer.rs`）——全部改调它。`repo.rs` 自己那份谓词消失（其 evil-input 断言迁入新 module 的表测试），`peer.rs` 的 `matches!` 大小写敏感问题随之消失：`LOCALHOST` 四处从此一个答案。同时建立**跨语言共享 fixture**（输入→期望的 JSON 表），Rust 表测试读它逐行断言。这笔是结构移动（无行为变化，除 peer 处的大小写修复），独立提交。

**Blocked by:** 01（先有独立可回滚的缺陷修复，再做结构移动）

**Status:** done（2026-09-23）

- [x] `crates/core/src/host_policy.rs` 落地：单函数 `is_loopback`，语义 = 决策 246 的规范形态（与 01 就地修的那份同源——02 把它搬进 module，不是再写一份）
- [x] 三个消费者改调 `host_policy::is_loopback`；`repo.rs` 原有谓词删除，其「前缀伪装不算回环」断言迁入新 module 的表测试
- [x] `peer.rs` 的绑定判定改调同一函数：`LOCALHOST`（大小写）与尾点形态与另外两处一致
- [x] 共享 fixture 建立：一张输入→期望的 JSON 表（路径形态照 `tests/fixtures/` 既有先例，Rust 写、TS 读），Rust 表测试逐行读它断言，覆盖决策 246 全输入表：放行/拒绝两侧含 `localhost.`、`LOCALHOST`、`127.999.999.999`（拒，八位组范围）、`0.0.0.0`、`::ffff:127.0.0.1`
- [x] grep 全仓 Rust 侧只剩**一处**回环判定实现（`127.` 前缀匹配的写法在 Rust 里零残留）
- [x] 01 的三条回归与四处既有回环用例仍绿；`make check` 绿
