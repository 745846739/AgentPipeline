# 03: 启动恢复接通崩溃路径

**What to build:** 服务被强杀后重启，agent 节点从**最后一条已记录的消息**接着跑，而不是空对话重跑；
已经做完的工具副作用不再重做。**这是整件事的用户可见目标。**

**Blocked by:** 02（续接源与「进程重启」原因）

**Status:** ready-for-agent

- [ ] 启动恢复在把遗留 run 收终态的同时，给中断的 running 游标置上「进程重启」续接原因——
      经既有的第二把钥匙（超时梯子用的同一条路，决策 320 的先例），不新开置位口
- [ ] **推翻一条钉住的测试，并记明为什么**：`restarting_does_not_record_a_resume_cause`
      （`crates/core/tests/integration/cursor_lifecycle.rs:1059`，断言文案「重启不是人的介入，
      不该留下续接原因」）当初是**故意**钉住「重启不置位、不另立「重启」这个原因」的
      （见 `.scratch/resume-semantics/issues/01-cause-driven.md` 的 ④）。本票把它改成断言
      「置原因、且该原因在判定表里判为续接」。**改之前先取证**：确认它今天钉的确实是「不置位」
      这件事，而不是别的副作用，并在票的 Comments 里写明推翻的理由
- [ ] 真二进制用例：kill -9 落在**工具调用中途** → 同一 home 重启 → 断言新 run 带继续链接、
      转录含被杀前已完成的消息、**已完成的工具没有被重放**（用一条 append 型命令，
      断言它在工作区里只留下一次痕迹）
- [ ] 四个杀点各一条断言：落在轮边界、落在某个工具调用中途、落在尾条终结消息之后、
      落在 post_process 期间。后两个杀点断言的是「不会做多余的事」（尾条之后不应重发模型调用
      而应直接进收口；post_process 阶段的重入按既有的 upsert 语义安全）
- [ ] 重启**不**重置工作区，并有一条测试或显式说明钉住这条分界——人按重试才做 `git reset --hard`
      + `git clean`（决策 125），重启恢复不是重试
- [ ] 不加新的可测试性接缝：五条接缝的条数被决策钉死（决策 143 / 194）。杀点靠真二进制 +
      脚本化模型 + 让某一步挂住，沿用既有的进程边界用例模式（决策 152）
- [ ] 决策日志追加：确认决策 127 / 162 的启动恢复序列多了一步「给中断游标置续接原因」

## Comments

**2026-10-08 实现者（推翻那条钉住的测试的理由，票面第二项要求写在这里）**

改之前先取证的结果，逐条交代：

1. **它今天钉的确实是「不置位」这件事**。用例原名 `restarting_does_not_record_a_resume_cause`
   （`crates/core/tests/integration/cursor_lifecycle.rs`），正文只做两件事：`set_task_status(Running)`
   → `requeue_running_tasks()` → 断言 `requeue_running_tasks == vec![task_id]`（归队生效）与
   `take_cursor_resume_cause(cursor) == None`（**原因列为空**）。第二条断言，加上用例头那段
   「重启不是人的介入，不该留下续接原因」的文案，钉的就是「重启不置位」。取证没有发现任何
   别的副作用被它顺带钉住：它不建 run 行、不碰 `cancel_origin`、不看超时梯子。
2. **推翻的是那个结论，不是「写实它」这个做法**。原用例自己的注释写着「写实它，是为了让那次
   重构在这里现形」——现在要实的是**相反**的事实，理由同样写进断言：`restarting_records_a_continuation_cause_for_agent_cursors`
   断言「原因真被置上」+「该原因在判定表里判为续接」+「取走即清零的登记语义不变」。
3. **前置的语义拆分**：原裁决把「重启不是人的介入」与「重启不该续接」当成同一件事。本票把
   两件事拆开——重启**仍然不是**人的介入（这一点一个字不改：`trailing_timeout_streak` 照旧
   跳过 `cancel_origin='restart'`，既不计数也不清零），但它**是**一条续接边界。不置位的后果是
   被强杀的那个 agent 节点拿一份空转录从头重跑、把已完成的工具副作用再做一遍——那是本功能
   要根除的东西。
4. **补一条闸**：新增 `restarting_leaves_code_node_cursors_unmarked`，钉住「纯代码节点不置位」
   （它没有转录可续，标记会永远无人取走、悬在列上）——与原用例同一道闸的两面。
5. **票面四个杀点只落了「工具调用中途」那一个真二进制用例**（`kill_9_mid_tool_call_resumes_from_the_log_without_replaying_completed_tools`）。
   另外三个的处置与理由：
   - **轮边界**（日志停在「一轮完整、下一轮模型调用在飞」）：机制面由
     `context.rs::completing_a_half_round_makes_the_transcript_legal` 的 `done == 3` 那一档
     （一轮齐全 → 补 0 条）与 `run_ledger.rs::the_log_is_immune_to_conversation_truncation`
     （完整转录原样交出）钉住。真二进制那一档需要「让模型调用挂住」的提供方（`MockLlm`
     只能瞬时回包），造法成本远高于它多验到的那一点点。
   - **尾条终结消息之后** 与 **post_process 期间**：票面原写的期望是「不应重发模型调用而应
     直接进收口」。**这条期望与本方案定下的恢复语义不符**——spec 明确选了**重放式**
     （「把日志读成消息数组当作承接内容交给新一轮 attempt，模型自己接着走。不回到循环里机械
     续跑剩余工具调用」）。重放式下这两处重启后**会**再发一次模型调用，那是有意的代价；
     真正该断言的是「不做多余的事」的另一半：**已完成的工具副作用不重做**（改由上面那条
     真二进制用例的 `MARKER.txt` 只出现一次来钉）与 **post_process 的重入安全**（`stage_outputs`
     按 `(task_id, stage, output_type)` upsert，既有语义，本轮未动）。
