# 01: 收口行带机器读出的改动清单

**Status:** done（已实现，决策 411；`cargo test --workspace` 1801 通过 / 0 失败、`clippy -D warnings` 与 `fmt --check` 全绿——落地记录见文末）
**Blocked by:** None (can start immediately)

**What to build:** 作为值班经理，我要**收口行自己说清这一轮动过哪些文件**——因为它是值班长
**下一轮**认识「我做过什么」的唯一读物，也是我读台账时唯一能看见的东西。今天两边都会撒谎：
394 那一轮改了二十余处，台账正文只有 104 个字，下一轮于是汇报「仍未动一行代码」。

**为什么**（因果链，三处症状其实是一件事）：

1. 历史进 prompt 的**只有 `message.content`**，工具痕迹从不回灌，窗口按字符裁
   （`conversation.rs::trim_history`，`FOREMAN_HISTORY_BUDGET_CHARS = 24_000`）；
2. 判停那一支的收口句是**硬编码模板**，不含改动清单（`runner.rs:1368`）；
3. 于是下一轮读到的「我做过什么」= 上一轮那句「这一轮我就停了」→ 它如实回报「本轮零改动」
   （`kanban_foreman_messages.id=400`）→ 人再说「继续」，它从零重新定位（42 次 `list_dir` 的来处）。

**这是决策 311 的下一代。** 决策 311（票 `foreman-burns-without-guard/issues/03`）已经立下
「收口句按**实际**说话」这条纪律，并留下口子：「排查同一收口路径里是否还有别处默认『提议必然存在』
……不代表只有这一句」。本票补的是同一类：**收口句对「本轮干了什么」的断言，今天只有提议数一个来源，
而改动文件完全没有**。

**形状**：

1. **清单从哪来**（不引入新的 IO）：
   - 默认：从本轮 `traces` 里取 `edit_file` / `write_file` 的 `args` 路径，去重、保首次出现序
     （394 那轮二十余处全在里面）；
   - 本轮若调用过 `repair`，**并上**那份权威 diff 的文件清单（库里 `payload_json.diff`，
     不读盘上那份 `.diff` 抄本）——`run_command` 里也可能改文件（`git apply`、`sed -i`），
     那一支只有 diff 看得见。**是并集不是替换**：痕迹只看得到两个编辑工具，diff 只看得到
     repair worktree 里那一份提交，谁缺了谁并集都补得上；顺序上痕迹在前（那是这一轮的实际发生序）；
   - 两者都没有（本轮没动文件）：**不出现这一行**（「没有」与「有但是空的」是两件事，与
     `briefing_json` / `traces_json` 同一条口径）。
   - **档位不是 `auto` 时整轮为空**（实现时定的第二处判据）：值班长的**缺省档位就是 `ask`**，
     而那一档下 env 写工具**落成提议、文件根本没动**（回执还是一句 `ok = true` 的
     「已生成一条待确认的提议……这件事没有执行」）。把那种痕迹算成改动就是伪造一份成果——
     正是本票要停掉的那类谎。`deny` 档同理（连工具都没给）。`ok = false` 的调用也不进清单。
2. **正文那一行**（机器生成，不由模型自己说——先例是 `FOREMAN_WATCH_MARK` / `OPERATION_LOG_MARK`
   两处后端追加，以及那句设计原话「播报的标记由后端加上，不由模型自己说」）：

   ```
   【本轮改动】本轮改了 12 个文件：frontend/src/lib/actions.ts、frontend/src/routes/Talk.svelte、
   frontend/src/routes/TaskDetail.svelte（余 9 个见台账明细）。
   ```

3. **明细落成结构化字段**：**新列 `changed_files_json`**（一条迁移）。判据留在后端、前端只渲染，
   与既有姿势一致。
   另两条出路**已否决、如实记**：(b) 复用 `segments_json` 新增一种段 kind——不加迁移，但它会进入
   有序段序，前端要按 kind 分支；(c) 前端自己从 `traces_json` 推导——零后端改动，但把判据挪到了前端。
4. **「零改动」断言拦截**：清单**非空**而本轮正文里出现「零改动 / 一行未改 / 仍未动一行代码」时，
   机器在正文末尾**补一句更正**并记一条 `tracing::warn!`——**不改写模型的话**（与在打转 / 成本告警 /
   归因三处「标注而非改写」的姿态一致）。

**验收**：

- [ ] L1 单元：汇总器——去重保序 / 只认编辑类工具 / 空 traces 回空 / 同一路径多次编辑只算一次
- [ ] L1 单元：断言检测器——「清单非空 + 正文称零改动」命中；「清单为空 + 正文称零改动」不命中
- [ ] L2 集成：一轮里改过文件 → 收口行的正文含 `【本轮改动】`，且计数与 traces 一致
- [ ] L2 集成：清单非空而正文称「零改动」→ 收口行带更正句
- [ ] L2 集成：一轮没改文件 → 正文**不出现** `【本轮改动】`
- [ ] L2 集成：**上一轮改过文件的收口行进得去下一轮的 prompt**（本票的全部意义，必须单独钉）
- [ ] 手工面：拿事故账班次 `01M4CDY9EC9M9FSSE236GJXYZC` 的数据重演——394 那一行的收口句应当
      列出它改过的文件；400 那一行不应再出现「仍未动一行代码」

---

**落地记录（2026-10-08，决策 411）**：

- 迁移 `0045_foreman_changed_files.sql`：`kanban_foreman_messages.changed_files_json`（路径数组，`NULL` = 没动过文件）。
- 汇总与措辞收在 `crates/core/src/pipeline/foreman/changes.rs`（新模块）：`changed_paths` / `union` / `diff_paths` / `claims_no_change` / `changes_note`。**两条判据是实现时才定下来的**，都写进了票面「形状」：① 档位不是 `auto` 时整轮为空（`ask` 是值班长的缺省档，那一档下写工具只落提议、文件没动）；② `ok = false` 的调用不进清单。
- 收口处接线 `runner.rs::respond_inner`：痕迹 ∪ 本轮 repair 的 `payload_json.diff`（库里读，不读盘上那份 `.diff`），正文末尾附那一段、声称零改动时换更正开头；行上落 `changed_files_json`。
- 存储层两处写入补列（`append_foreman_message` / `close_foreman_inflight`）与 `FOREMAN_MESSAGE_COLUMNS`；`storage/proposals.rs` 加 `round_repair_diffs`。
- 线上加恒在场的加性字段 `changed_files`（`crates/app/src/routes/foreman.rs::message_wire`）——界面这一批不消费，前端另立票。
- 验收：L1 11 条（`changes.rs` 内联）+ L2 4 条（`tests/integration/foreman.rs` 末尾一组）；`cargo test --workspace` 1801 通过 / 0 失败（7 ignored）。
- **与票面的两处偏差**（已回写进上面的「形状」）：diff 那份来源做成**并集**而不是替换；补上「档位不是 `auto` 就不算改动」这条判据。
- **未做**：前端显示（另立票）；票 02（五条收场尾句都按实际产出说话）**Blocked by 本票**，已于 2026-10-09 落地（决策 414）。
