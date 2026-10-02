# 06: offload_run 工具——开关控可用,gh 触发+轮询+文本回读,失败回退留痕

**What to build:** 开关开启后,agent 获得一个显式的外发工具:把**已提交状态**
(分支 + commit SHA)交给 04 的工作流,`gh workflow run` 触发、轮询至收场、
文本结果(退出码+日志尾部)作为工具结果回读;工具结果语义与本机命令执行对齐
(超时、退出码、失败可见)。**失败回退本机执行但留痕**:WARN 日志 + 任务事件可见
「本次外发失败,已本机重跑」+ 设置页探测显红——降级可以,静默不行
(与决策 185/297 及 silent-degradation 的一贯立场一致)。开关关闭时工具不可用,
一切照旧本机跑。是否外发由 agent 按任务指导自判(全量 test/clippy 值得等排队,
快测试不值得)。

**Blocked by:** 04, 05(均已落地;106 真实外发的验收项依赖 02 完成)

**Status:** ready-for-agent

> **实现笔记(2026-10-02,开票时的勘探结论,动手前先读)**:
> 1. **工具形态**:agent 传 `{command}`——与已落地的 offload.yml 需配套改一版:
>    工作流从「三个固定 job」改为带 `command` 输入的单 job(工具侧白名单校验:
>    仅 `cargo test` / `cargo clippy` / `cargo build` 前缀,且不含 `;`/`&&`/管道/
>    命令替换)。白名单是防注入面,工作流侧不再重复校验。
> 2. **目录表连动**(决策 353):加 `offload_run` 要动 `TOOL_SPECS` 的**数组长度**
>    (8→9)与三处冻结断言:`all_parameter_texts_are_valid_json_objects`、
>    `advertised_schemas_match_execute_parsing_field_by_field`、以及「内置集里
>    动手的恰是四个」那条——offload_run **不该**进 `is_env_tool`/`ENV_WRITE_TOOLS`
>    (它推远端,不走环境写层),先读 `catalog.rs` 的测试再动手。
> 3. **档位**:`gate_decision`/`denied_by_tier` 里 offload_run 建议对齐 run_command
>    的档位语义(Execute 档直行)——推 kanban 分支是例行动作,不设确认钮。
> 4. **开关判定**:工具开头懒读 `offload_switch()`(经 `rtk_store` 那条 Store 线),
>    关 → `Error::Validation("外发未开启…")`。
> 5. **未提交校验**:git.rs 已有「N 处未提交(其中 M 个未跟踪)」的只数助手
>    (约 379 行注释处),复用它;有未提交 → 明确报错,不静默快照。
> 6. **测试缝**:gh/git 都是真子进程(工具真跑哲学)——测试在临时目录放**假 gh/git
>    脚本**,经 `ToolExecutor` 的一条 PATH 前置注入(可仿 ChildEnv path_prefix 的
>    形状加一个 `with_command_path_prefix` 构造项);假 gh 首轮即回 completed,
>    轮询不 sleep,测试秒级。轮询循环的间隔与总上限(建议 60min)用常量钉。
> 7. **回退留痕**:任何外发步骤失败 → `tracing::warn!` + 本地执行同一条命令
>    (复用 `run_command` 分支,显式超时 1800s)+ 工具回执里写明「已回退本机」。
>    票面写的「任务事件」暂以回执文本代替(回执进转录),不另开事件通道——
>    动手时若发现现成的事件口子,用之,并在本票记录。

- [ ] 开关开:agent 经工具完成一次真实外发(106 上,依赖 02),结果文本完整回读
- [ ] 开关关:工具不可用(拒绝并说明),既有命令路径零变化
- [ ] 外发失败(凭据失效/排队超时/结果拉取失败)→ 回退本机执行,任务不中断,
      且 WARN + 任务事件 + 探测读数三处留痕
- [ ] 只外发已提交状态:工具入口即校验「无未提交改动」,未提交时明确报错而非静默快照
- [ ] 外发整链路有超时上限,不会把 agent 循环挂死在轮询上
