# 01: develop/review/test 出厂默认 max_duration_sec 提到 5400

**What to build:** 从用户视角：重读型阶段（develop / review / test）的节点在模型持续干活
（流式心跳正常、idle 闸不触发）时，不再被 30 分钟的全局默认硬墙反复击杀——出厂即给
90 分钟（5400 秒）预算；其他阶段维持 1800 秒全局默认不变。

**为什么**：ux-audit-3（任务 01M450DK2GKZP4PJ4FVRAFAGXZ）的 7 小时浪费全部来自
「模型一直在干活但 30 分钟不够用 → 跑满被杀 → 续接 → 再跑满」的循环。决策 66 的双闸
语义本身没错（idle 闸管挂死、max_duration 闸管 runaway），错的是预算档位与阶段形态
不匹配：develop/review/test 这三个做实事的阶段，30 分钟对重读型任务（通读源码、走查
页面、跑 e2e）根本不够一段。106 上已手动把三阶段调到 5400 并验证有效（调大后
test.execute 第 9 次一口气 76 分钟跑通），但那是**运行时配置**，换台机器部署就回到 1800。

**形状**：

- `config.rs` 新增**出厂阶段默认**函数：`develop` / `review` / `test` 三阶段缺省
  `max_duration_sec = 5400`，其余阶段不出厂值（回落全局 1800）。
- 接线点在 scheduler 的超时判定处（`scheduler/mod.rs` 有效值计算）：阶段级覆盖取值
  改为 `stage_cfg.max_duration_sec（DB 覆盖）or 出厂阶段默认 or 全局`——**三级优先序
  不变（决策 66：节点 > 阶段 > 全局）**，出厂默认插在「DB 阶段覆盖」与「全局」之间，
  不新增层级、不改 `effective_max_duration` 签名（在调用侧先归并出厂值）。
- `conversation_max_chars` 一类落库/压缩语义不碰；idle 闸（300 秒）不动；梯子
  （决策 320/205）不动。
- 106 上已有的 DB 阶段覆盖（5400）与本票出厂值相同，**部署后 106 行为零变化**；
  受益的是其他环境与全新部署。

**Blocked by:** None

**Status:** done

- [x] 单测：出厂默认函数对 `develop` / `review` / `test` 返回 5400，对其余全部阶段
  （init / architect-design / develop-design / test-design / sync-check / merge / done）返回 None
- [x] 单测：DB 阶段覆盖仍**压过**出厂默认（设了 1800 就用 1800，不是 5400）；
  未设时回落出厂默认；节点级覆盖仍压过一切（三级优先序回归）
- [x] 决策日志续写一条（照 README「决策」小节的口径，修订关系：不推翻决策 66，
  补的是出厂档位；与决策 320 的梯子无涉）
