# 02: token 计数改「真实账」——删去双算排除 + 投影滚动入账(B3/B6)

**来源:** 同 01 的监控实录。两个观测:① 长节点执行期间 `/tasks/{id}` 的
`updated_at`/`total_tokens` 完全冻结(静止 45 分钟,差点被监控误判成卡死),
节点收口瞬间一次性入账 +1400 万 token;② 超时续接后 `/tasks/{id}/metrics` 的
`total_tokens`(活计数)回落到旧值,与 `stored_total_tokens`(新值)打架,
活计数一度比落库值小 1400 万。

**根因:** ① `refresh_task_totals`(`core/src/storage/tasks.rs:407`)只在
attempt/节点边界被调(`model_invoke.rs:286/312/337` 等收口点),长节点的百余次
模型请求全部压到节点结束时一次入账;② `metrics.rs::total_tokens` 按
「被后继 `continued_from_run_id` 指认」整条排除历史 run(决策 180,票 13)——
被排除的量正是模型真实烧掉的钱,且排除随续接梯子逐档下跌,读数与落库值分叉。

**Blocked by:** None

**Status:** done

- [x] 删除 `total_tokens` 的排除规则,改盲求和(真实账语义);模块注释写明
      推翻决策 180 的证据链
- [x] 新单测 `total_tokens_keeps_continued_from_runs` 钉住新语义
- [x] 集成测试 `continued_run_links_back_so_tokens_are_not_double_counted`
      改名为 `..._and_its_tokens_count_as_real_cost`,断言 counted == naive
- [x] `continued_from_run_id` 只做谱系:run_ledger / types 两处注释改口径
- [x] 投影滚动入账:agent 轮循环里搭心跳的便车(`touch_run_heartbeat` 处),
      `USAGE_PROJECTION_REFRESH=30s` 节流,失败不拖累这一轮(真相源在台账)
- [x] core lib / integration / app lib / app integration 全绿,clippy 全绿

**边界.** 不改写频率为每请求一次(SQLite 写竞争带,1.4s 慢语句前科);
不把 30s 提为配置项(实现细节,等真有需求);`total_calls` 口径不动;
不给「被排除量」单独开展示列(排除已不存在,无东西可展示)。

## 实施记录(2026-10-02)

落点:`crates/core/src/metrics.rs`(口径 + 注释)、`crates/core/src/pipeline/model_invoke.rs`
(`USAGE_PROJECTION_REFRESH` 常量 + 轮循环内节流刷新)、`crates/core/src/pipeline/run_ledger.rs`
与 `crates/core/src/types.rs`(注释改口径)、`crates/core/tests/integration/executor.rs`
(测试改名与断言翻转)。
