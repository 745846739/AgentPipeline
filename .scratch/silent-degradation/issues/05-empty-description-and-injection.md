# 05: 入口拒绝空描述 + 把 validate_input 的问答注入下游

**What to build:** 两件事，治的是同一个病：**下游节点在"知道要干什么"这件事上一无所有**。

事故现场：`kanban_tasks.description = ''`，任务目录里 `user-input.md` 只有两个字「同意」。
architect-design.execute 收到的 user prompt 是

```
任务标题：修复前端闪屏问题
任务描述：
…
## 用户补充输入
# 用户补充输入

同意
```

它**看不到"同意"同意的是哪两个问题**。决策 277 那套"问题 + 推荐答案"的意义恰恰在于
**后面的节点能看到它**（`routes.rs:16` 的注释写得很清楚：`info_insufficient` 的 pending
消息带问题清单），但那答案只活在 validate_input 的会话与 pending 消息里，
重入段（`crates/core/src/pipeline/model_request.rs:415-444`）只注入
`backtrack-feedback.md` / `user-input.md` / `retry-feedback.md` 三个文件的**原文**。

结果就是这个任务的行为：60 分钟、89 次只读调用、一个字没写——**它在靠翻仓库猜范围**。

① **入口拒绝空白描述**。`CreateTaskBody.description` 是 `#[serde(default)]`
（`crates/app/src/routes/tasks.rs:43-44`），`NewTask::new` 把 description 初始化成空串
（`crates/core/src/storage/tasks.rs:33`），创建处理函数原样接收（`:101-102`）。
改为：`title.trim()` 与 `description.trim()` 都非空才放行，否则 400 并说明原因；
**重提路径同样要过这一关**（`crates/app/src/routes/tasks.rs:592` 那处从旧任务再建一条）。
前端创建表单同步把描述标为必填，别让用户撞到后端 400 才知道。

② **把问答落下来并注入下游**。最小改动、且不给重入段再加第四个文件：
在 `info_insufficient` 被答复、任务恢复时，把**问题清单 + 推荐答案 + 用户答复**
一起写进 `user-input.md`（现在只写用户答复原文）。这样三件事一次到位——
可追溯（台账里能看见"同意了什么"）、注入（execute 现成的重入段自动带上）、
以及"首轮为空不渲染"的既有语义不变（仍然只在有内容时渲染）。

**Blocked by:** None

**Status:** ready-for-agent（2026-10-01；批次二）

## 落点

- `crates/app/src/routes/tasks.rs:58` `create`：空白校验；`:592` 重提路径同样处理。
- `crates/core/src/pipeline/resume.rs:179-230`：`is_info_insufficient` 那条分支现在只把
  用户答复写成 `user-input.md`；要改成把问题清单（来自 pending reason 的
  `MetadataView::readiness_with_blockers`，`crates/core/src/pipeline/model_invoke.rs:1533-1535`）
  与答复合成一份。
- `crates/core/src/pipeline/model_request.rs:415-444`：确认注入仍读同一个文件
  （若改文件名，三处重入段要一起改）。
- 前端创建表单（`frontend/src/routes/*` 里的新建任务处）同步必填。

## 验收

- [x] `POST /tasks` 带空 `description`（空串或纯空白）→ **400**，报文说明描述是必填
- [x] 重提路径（从旧任务再建）同样被拦；正常创建不受影响
- [x] 单测：伪造一次 `info_insufficient` pending → 答复 → 落库的 `user-input.md`
      **同时含**问题原文、推荐答案与用户答复
- [x] 单测：architect-design.execute 的 user prompt 渲染结果里能看到这三样
      （拿本次事故的真实问答做 fixture：Q1 症状 / Q2 验收）
- [x] 回归：首轮（没有 user-input.md）不渲染该段，既有 prompt 快照不漂
- [ ] `make check` 全绿（本地按决策 331 跑 `make check-lint` + 改动层的窄跑，全量在 CI）

**明确不做**：不改决策 277 的"问题 + 推荐答案"契约；不把问答塞进 `stage_outputs`
（它是给 prompt 用的中间物，不是阶段产出，塞进去会污染 sync-check 的元数据口径）。

**来源：** `.scratch/silent-degradation/spec.md` 放大器一；现场 prompt 原文见
`kanban_node_conversations` id 102 的 `user_prompt`。

## 落地记录（2026-10-01）

### ① 入口拒绝空白
- `crates/app/src/routes/tasks.rs::create`：`title.trim()` 与 `description.trim()` 皆非空才放行
  （在项目存在性检查**之前**判——那是输入形状问题，不必先查库）；报文直说哪一格必填。
- `split`（重提路径，`:592` 那处）逐个子任务同样判，且判在**任何写操作之前**——
  否则原任务会先被置 `cancelled` 才报错。
- **波及面如实记**：7 处既有测试夹具原本不带 `description`，一并补上（`api_contract.rs` 六处 +
  `smoke.rs` 一处）；其中两处是"断言别的 400"的用例（未知依赖 / 未知项目 / 无 provider），
  不补的话会先撞上描述校验、断言失效。
- 前端两处同步必填：`NewTaskDialog.svelte`（`fieldError` 的字段联合加 `'description'`，
  `aria-invalid` / `aria-describedby` 接上，标签写「描述（必填）」）与
  `SplitDialog.svelte`（每行格式「标题 | 描述」两者都必填，缺描述时按行号报错）。

### ② 问答注入
- `pipeline/resume.rs` 的 `is_info_insufficient` 分支：不再只写用户那两句话，改成
  「`## 当时提交的问题（含推荐答案，来自 validate_input）` + pending 消息（决策 277④ 的问题清单）
  + `## 用户答复` + 答复原文」。
- 两个下游通道都自动带上：execute 侧走重入段渲染（决策 79 的 `user-input.md`），
  validate_input 侧走决策 279 的 `supplement_input`——它剥的是首行 `# 用户补充输入`，
  新加的几节正文照样进 user turn。
- **没加第四个注入文件**（票面的最小改动口径），文件名的三处引用一处未动。

### 测试
- `crates/app/tests/integration/api_contract.rs`：`create_task_rejects_blank_title_or_description`、
  `split_rejects_a_child_with_a_blank_description`。
- `crates/core/tests/integration/executor.rs`：`an_info_insufficient_answer_is_recorded_together_with_its_questions`
  （答完之后再让 validate_input 通过，读 architect-design.execute 那条请求的 user prompt，
  断言三样都在）。
- 前端：`NewTaskDialog.test.ts`（描述空 → 不提交、错落在描述那格、填上就放行）、
  `SplitDialog.test.ts`（无描述的行被拦下并按行号报；两处既有用例的输入补了描述）。

### 收尾时被既有测试抓出来的一个连带（值得记）
把问答写进同一个文件之后，**决策 279 的 user turn 契约被打破了**：`supplement_input` 原来的
口径是「剥掉首行 `# 用户补充输入`，剩下的就是用户说过的话」，而我新加的两节正文正好落在那
「剩下的」里——于是 validate_input 续接时塞进转录的 turn 变成了整篇留痕，而不是用户那句话。
两条既有用例当场变红（`supplement_input_rides_the_transcript_tail_and_leaves_the_first_message_verbatim`
断言 turn 与答复逐字相同；`a_second_resume_without_new_input_does_not_replay_the_old_supplement`
依赖同文比对做去重）。

改法：`supplement_input` 在正文里认 `## 用户答复` 小节，**有就只取它**，没有（旧格式文件）
就整段当答复；补一条单测钉住。留痕里两节都留着（可追溯 + execute 侧重入段照旧渲染全文），
只有 user turn 恢复成"用户说过的那句话"。

### 落地后补记：波及面还扩散到 e2e（提交 631b202）
上面那七处夹具是 `cargo test` 能抓到的，**抓不到的在后一层**——e2e 自己经 `POST /tasks`
播种（`frontend/e2e/harness.ts` 两处），也给新建对话框填表。空白描述闸门一上，CI 的 e2e
作业 38 条全红、`deploy-106` 被跳过（决策 330 的形状：check 红就没有部署）。
补齐四处：`harness.ts` 两个播种口、`create-flow.spec.ts` 的 ⑤、
`ux2-flows-and-copy.spec.ts` 的 ②（两个对话框）、`modal-keyboard.spec.ts` 的 ① 与它那条
候选任务夹具。教训与票面同源：**「必填」这类入口收窄，播种路径与填表路径要一起数**——
`cargo test` 与 vitest 都看不见 e2e 的播种口。

**未收口的一处（如实记）**：这条闸门只管**新**任务。事故任务
`01M3QW8CKS07R3MWG9XM4FNYER` 的 `description` 长度是 0（建于 2026-09-30，早于本票），
它照旧能续跑——要不要给它补一句描述、或就此作废重建，是用户的决定，不是本票能替的。
