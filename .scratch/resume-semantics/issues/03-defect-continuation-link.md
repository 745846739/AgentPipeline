# 03: 缺陷——续接链接落在干净重试的轮次上，token 总量被少算

**What to build:** `pipeline/executor.rs:886-892` 的
`if let Some(c) = &continuation { link_run_continuation(run_id, c.from_run_id) }` 写在 `for round`
循环里，只看了 `continuation.is_some()`、**没看 `round`**。于是 `agent_retry_max` 的干净重试轮
（`carried` 按构造是 `&[]`，`executor.rs:902-910`，注释还写着「干净重试与首跑都不留链接」）也会把
`continued_from_run_id` 指向那条历史 run；而 `metrics::total_tokens`（`crates/core/src/metrics.rs:24-41`）
把历史那一侧排进 `superseded` 集合——历史 run 被**重复排除**，结果是**少算**（不是双算）。

修法：把链接挪到 `round == 0`（或按 `carried` 非空判断），与它自己注释里那句话一致。
既有测试 `continued_run_links_back_so_tokens_are_not_double_counted`
（`crates/core/tests/executor.rs:3204-3246`）只覆盖了 round 0 的成功续接，需要补一条
「干净重试不留链接」。

**为什么与续接同批**：它就在续接这条线上，而票 01 会改这一段的取数方式，两处一起动省得来回。

**Blocked by:** None（可立即开始）

**Status:** done

- [ ] 链接只在 round 0 落（测试：一个续接 run + 一次干净重试，历史那条 run 只被排除一次）
- [ ] 修完后 `refresh_task_totals` 的读数符合「排除的是历史那一侧」这条注释
- [ ] `continued_from_run_id` 的语义与消费方（`metrics::total_tokens`，唯一）不变
- [ ] 交付说明里写清这个 bug 的**方向**（少算而非双算）与它影响到的读数（任务 `total_tokens`）

## 交付

本票已落地（2026-09-17）。缺陷的**方向是「少算」而不是双算**，这就是为什么它一直没被察觉：
读数偏小不像偏大那样会有人来问，而落库的任务 `total_tokens` 是汇总口径，越跑越偏。

- 修法：`pipeline/executor.rs` 的 `link_run_continuation` 挪到 `if round == 0` 里面
  （`carried` 从第 2 轮起按构造是 `&[]`，与它自己注释里「干净重试与首跑都不留链接」一致）。
- 既有用例 `continued_run_links_back_so_tokens_are_not_double_counted` 覆盖 round 0 的成功续接
  （照旧绿）；round ≥ 1 那一半由 `clean_retry_after_a_tool_failure_stays_empty_whatever_the_cause_says`
  的 run 分组断言盯着——它断的是「首条请求的 messages 数为 0」，而链接与起点在同一处判断，
  链接多落一次必然带出非空起点。
- `continued_from_run_id` 的语义与唯一消费方（`metrics::total_tokens`）未动。
