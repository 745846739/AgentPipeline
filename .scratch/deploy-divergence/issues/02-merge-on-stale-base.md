# 02: 合入落在一份陈旧基线上——基准判定读的是没人刷新过的本地引用

**Status:** ready-for-agent
**Blocked by:** None (can start immediately)

**What happened（2026-10-08，与 [01](01-repair-commit-blocks-deploy.md) 同一场事故的另一半）**：

- 11:08:26 那条「合入修复分支 → `main`」执行后，106 的检出 `/opt/AgentPipeline` 的 `main`
  停在 **`ba77558`**（`[repair] 值班长修复 01M4CEFYE8XBZ3R98TKDFY3ZCV…`，parent **`d4e11b9`**），
  `git rev-list --left-right --count origin/main...main` = **6 1**。
- 于是部署继续失败在 `git pull --ff-only`（与 01 同一个失败面，但**成因不同**：01 是本地多了一条提交，
  本票是**那条提交站在一份 2.5 小时前的基线上**）。
- 时间线：修复 worktree 于 **08:25** 从当时的 `origin/main`（尖端 `d4e11b9`）切出；到 **11:01** 落提交时，
  `origin/main` 已经前进了 6 个提交。
- 那张合入提议的 `situation_json` 里**明记**着 `base_commit: "d4e11b9…"`、`base_ref: "origin/main"`
  ——而**执行时一次都没读它**。

**根因（不是少了校验，是读数陈旧）**：

1. `run_repair` **有**「先 rebase 再合入」这一步（决策 212①「指纹换义为能不能干净 rebase」），
   注释也写着「分支不会因为别的事变迁而失效，会变的是**基准**」。
2. 但 rebase 的基准是从**本地 `origin/main` 引用**解析出来的。而**全仓唯一的 `fetch` 在建 worktree
   那条路上**（且那里明确「fetch 失败不阻断」）——`repair.rs` 与 `foreman_actions.rs` 里
   **没有任何 fetch**。
3. 于是 11:08 那次判定读到的是**从 08:25 起就没人刷新过的** `origin/main`（仍是 `d4e11b9`），
   得出结论「HEAD 已是 base 的后代 → Clean（no-op）」，rebase 什么也没做；随后合入走**真 ff**
   （先判 `graph_descendant_of` 再写引用）把 `main` 移到了那条陈旧基线上的提交。
4. **它是静默的**：回执只说「闸门已过 / 已合入 … 并回收修复 worktree」，一个字没提 `main` 刚离开了
   `origin/main`。人读到的是好消息。

**形状**：

1. **抽出可复用的 fetch**，把建 worktree 里那 6 行搬出来两处共用——那里的注释自己写着
   「这些都不该有第二份」。这是本票的第一步（预构，不改变行为）。
2. **合入前 fetch**，且**失败即拒执**（与建 worktree 那处的「不阻断」相反）：这条路上「安静地拿旧引用
   判基准有没有前进」正是本次事故，离线时宁可不合。拒执文案说清「取不到 origin，不敢断言基准」。
3. **`situation_json.base_commit` 与 fetch 后的 `origin/main` 尖端比对**，把事实写进回执：
   「基线 `d4e11b9` → `a7f4785`，已 rebase 合入」。**不**要求人再按一次、**不**因基线前进而拒执
   ——rebase 已经保证结果正确，再加一道按键会让「白天有人推了 main」变成修复永远合不进去。
4. **回执补两个读数**：`main` 现在落在哪个提交、与 `origin/main` 的关系（今天只有「已合入」三个字）。

**为什么和 01 是两张票**：01 是**取材**问题（同一个目录同时扮演「部署产物源」与「可写项目仓」），
三条出路里的「独立克隆」能解它；本票是**读数**问题（基准新鲜度）——**即使取材拆开了，只要合入前
不 fetch，照旧会落在一份陈旧基线上**。

**验收**：

- [x] L2 集成：用裸 origin 夹具建 repair worktree → **让裸仓的 `main` 前进一条** → 合入 →
      断言 `main` = **新尖端 + repair**（而不是旧基线上的提交）——`tests/integration/repair.rs::a_merge_after_the_origin_moved_lands_on_the_new_tip_and_says_so`
- [x] L2 集成：同一用例断言**回执里出现「基线 … → …」**（同上用例，另钉「已 rebase 合入」「main 现在落在」「领先 origin/main」）
- [x] L2 集成：origin 指向不存在的路径 → **拒执**，且文案说清「取不到 origin」（不是「闸门没过」那种含糊话）——`an_unreachable_origin_refuses_the_merge_and_names_the_reason`（另断言 main 一个提交不动、worktree 与分支保留）
- [x] L1 单元（若抽出 fetch 时有纯函数可钉）：fetch 结果的判定（取到 / 取不到 / 远端不存在）各一条——**偏差**：fetch 本体不是纯函数，L1 钉的是回执纯函数 `merge_receipt_note` **六条**（位移两端 / no-op 不自称 rebase / 没动不提基线 / 读不到不编数 / 没动×读不到只剩裸落点 / 无 origin 仍报落点）；「取到 / 取不到 / 远端不存在」三态由下面三条 L2 真 git 用例覆盖
- [ ] 手工面：一次「修复轮 + 紧接着一次部署」连跑，两端都不需要人 ssh 上去清分支（与 01 的手工面共用）——**未做**（需要 106 现场，随部署同车验收）

**Status:** done（已实现，决策 415，2026-10-09）

## 落地记录（2026-10-09）

**实现**：`repair.rs` 两个新函数——`fetch_origin_tip`（合入前 fetch，--prune；三种读数：取到 `Some(tip)` / 无 origin `None` / 取不到 `Err`）与 `merge_receipt_note`（回执纯函数：基线位移句只在真动了时出现 + 落点 + 领先数读不到不编）；`foreman_actions.rs::run_repair` 在 rebase 之前接 fetch（失败 → `Error::Conflict`「取不到 origin（…），不敢断言基准是否前进——没有合入」，提议保持 pending 可重按），合入后回执补「基线 xxx → yyy，已 rebase 合入；main 现在落在 zzz（领先 origin/main N 个提交）」。

**与票面形状的两处偏差（如实记）**：

1. **形状 1「抽出可复用的 fetch（预构）」未做**：票面要求把 `init_worktree_named` 里那 6 行 fetch 搬出来两处共用，但施工时主树 `git.rs` 正被并行任务（决策 413）占脏——两批未提交改动同文件必然互相覆盖（上一票踩过）。fetch 因此住在修复域 `repair.rs`（语义上这里也确实与建 worktree 那处**相反**：失败不阻断 vs 失败拒执，共用一份反而要给它加一个「失败算不算错」的参数）。等 413 落地后再收敛，函数 doc 里已注明。
2. **L1 那条的形态**：票面写的是「fetch 结果的判定各一条」，实际 fetch 不是纯函数（真网络面），L1 改钉 `merge_receipt_note` 六条（评审后从四条补齐，见下），fetch 三态由 L2 真 git 用例覆盖（见验收第 4 条）。

**评审轮（code-review 双轴，2026-10-10）**：抓出三个已修的缺陷——① 回执把「已 rebase 合入」写成恒在，rebase no-op（远端回退、修复分支已含新基准）时会编一句没跑过的动作 → `rebased` 改由「rebase 前 HEAD vs rebase 结果的 head」实测（读不到按「没跑过」报），L1 补 `a_moved_base_that_needed_no_rebase_does_not_claim_one`；② 落点读数在 origin 缺席时整句消失 → 落点**恒在**、无 origin 仍报（L1 `a_repo_without_origin_still_reports_where_main_landed`）；③ **建 worktree 之后 origin 被摘**是事故读数的另一半，原实现静默放行 → `base_ref` 来自 `origin/…` 而 fetch 读不到 origin 时同样拒执（L2 `an_origin_removed_after_the_worktree_was_cut_is_also_refused`）。另按决策 412 的候选集不变量把措辞矩阵补到全格（L1 `an_unmoved_base_with_an_unreadable_ahead_reports_the_bare_landing`）。判断型气味（fetch 与建 worktree 那处的重复、`merge_receipt_note` 五参数据团）经权衡**有意保留**，理由记在函数 doc。

**验证**：L1 `repair.rs` 6 条新全绿；L2 `tests/integration/repair.rs` 3 条新全绿（裸 origin 夹具 + `remote set-url` 指向不存在路径 + `remote remove origin`）；合并新基线（5fa7476，含决策 413/416）后隔离 worktree 全量 `cargo test -p agentpipeline-core` **lib 828 通过 / integration 547 通过 · 4 ignored · 0 失败**，fmt + clippy（-D warnings）干净。手工面未做（见上）。
