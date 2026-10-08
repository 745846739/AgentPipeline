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

- [ ] L2 集成：用裸 origin 夹具建 repair worktree → **让裸仓的 `main` 前进一条** → 合入 →
      断言 `main` = **新尖端 + repair**（而不是旧基线上的提交）
- [ ] L2 集成：同一用例断言**回执里出现「基线 … → …」**
- [ ] L2 集成：origin 指向不存在的路径 → **拒执**，且文案说清「取不到 origin」（不是「闸门没过」那种含糊话）
- [ ] L1 单元（若抽出 fetch 时有纯函数可钉）：fetch 结果的判定（取到 / 取不到 / 远端不存在）各一条
- [ ] 手工面：一次「修复轮 + 紧接着一次部署」连跑，两端都不需要人 ssh 上去清分支（与 01 的手工面共用）
