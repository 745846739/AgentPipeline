# 01: 打断策略原语收敛 interrupt.rs

**What to build:** 决策 355——新文件 `crates/core/src/interrupt.rs`（纯函数、无 I/O）：
去抖窗口、按主体冷却、小时上限 + 上限通知去重三件原语。`foreman::watch` 的
`WatchFailureState` 退避与冷却段、`notify::resolve_politeness` 的同型判断改为调用原语；
两套词汇与对外行为零变化。头注写明「一次事件一次打扰」的唯一实现在此。

**Blocked by:** None

**Status:** done（已实现，决策 355）

- [x] `interrupt.rs`：三件原语 + 纯函数单测（窗口边界、冷却到期、上限触发 + 上限通知
      只发一次）——落成七个纯函数（见注记 ①），8 条单测逐条钉边界
      （`next_knock` / `waiting` / `debounce_elapsed` / `window_start` / `cooling_by_age` /
      `backoff_secs` / `over_hourly_cap` / `cap_notice_due`）
- [x] `foreman::watch` 去抖 / 冷却 / 小时上限段改调原语；`WatchFailureState` 的落库
      形状不动——`WatchFailureState` 是**纯内存**结构（无落库面），其字段一字未动；
      `delay_secs` 只留「选档」（哪一类用哪组 base/max），翻倍与封顶交给 `backoff_secs`；
      `waiting` 改调 `interrupt::waiting`；watch 闸门四处改调
      `debounce_elapsed` / `window_start` / `over_hourly_cap` / `cap_notice_due`
- [x] `notify.rs` 的 politeness 判断中同型部分改调原语；`notification_class` /
      quiet hours 判定留原处（那是 notify 自己的词汇）——`should_notify` 只换掉
      节流那一格（`!cooling_by_age(last_age_sec, cooldown_sec)`），
      `is_quiet_hours` 与豁免顺序一字未动
- [x] 验证：原语单测 + watch / notify 既有用例照绿；core 全量 + lint 绿——
      foreman 集成 136 条过、notify 集成 18 条过、scheduler_tick 43 条过、
      `notify` lib 38 条过；fmt / clippy 见提交闸门
- [x] 复演确认：决策 350 类触发面口径调整此后只碰 `interrupt.rs`（在头注里写明）——
      头注末节「这条线以后怎么改」明写适配器只负责取值 / 调函数 / 落账

**注记（留给后来者）**：

- ① **为什么是七个函数而不是三个**：决策 355 数的是**三件原语**（去抖 / 冷却 / 上限+去重），
  落地时要区分三种**读数形态**——比时刻（`waiting` / `next_knock`）、查库的左沿
  （`window_start`）、算好的间隔秒数（`cooling_by_age`），另加票面点名的**退避**
  （`backoff_secs`，此前是 `WatchFailureState::delay_secs` 里的翻倍算术）。三个概念、
  七个入口，一个常量都不持有。
- ② **两侧边界的缝是有意的，并且钉住了**：查库那一档（同任务冷却问「窗口内有没有被
  消费过」）走 SQL 的 `consumed_at >= 左沿`，比时刻那一档走 `waiting`（严格小于）——
  恰好落在左沿上那一个瞬点两侧结论相反。原样保留是为了「对外行为零变化」，
  差异写在模块头注并有一条单测（`the_db_side_and_the_clock_side_differ_only_at_the_left_edge`）
  钉住。想统一口径的人会先看见它。
- ③ **同任务冷却的时钟读数收成一拍**：原先循环里每件待办各读一次 `now`，现在整趟用一个
  `cooldown_floor`。真实时钟下差在微秒级（假时钟下逐字相同），方向是「同一趟用同一个
  现场」——与决策 356「时钟从快照取」同姿态。

## Comments

- 2026-09-30 实施完毕（决策 355）。两轴评审（Standards + Spec）各一轮，改动如下：
  - **Ordering 一处回退**：同任务冷却的时钟读数恢复成**逐件各读一次**（曾收成一趟一个
    `cooldown_floor`）——收起来会让左沿比原先更早、可能多算进一行已消费的待办，
    与票面「对外行为零变化」有出入。现在与从前逐字一致，只是左沿的算术住进了
    `interrupt::window_start`。
  - **公开面收窄**：`next_knock` 改私有（外加一个时长不是调用点需要做的判断；
    对外只有 `waiting` / `debounce_elapsed` / `window_start` / `cooling_by_age` /
    `backoff_secs` / `over_hourly_cap` / `cap_notice_due` 七个）。
  - **边界那条缝的措辞收紧**：原注记把两个读数说得像一处现役分歧，实际「含左沿」那一侧
    只住 watch 的同任务冷却（走 SQL），对照物 `debounce_elapsed` 与它是同一条算术——
    测试里补了「为什么对照物是它」。
  - 头注补了一节「这是一次搬家，不是改口径」，把「对外行为零变化」写进模块自己。
- 未改的评审意见（记下理由，留给后来者判断）：`is_watch: bool` 在三个签名间travel
  （Primitive Obsession）——它是本模块与整个 foreman 的既有词汇（`input.is_watch()`），
  本票不动。
