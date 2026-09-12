# 13: review_diff 产出与下发

**What to build:** 决策 124：人工评审模式下 review.execute 完成后由系统生成 `git diff {base_ref}..kanban/{task_id}`（记为系统来源的命令），写任务目录 `review-diff.diff` 并落 stage output（`output_type = review_diff`），经文件下发端点交给任务详情的评审面板。现在完全没有生产者——人工评审的待办里只有评审报告，没有 diff，评审人看不到改了什么。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] 人工评审 pending 前生成 diff 文件并落 stage output，记为系统来源命令
- [ ] 文件经既有文件下发端点可取（路径确定，覆盖写入可重入）
- [ ] 任务详情评审面板展示该 diff；无 diff 时降级不报错
- [ ] 用例覆盖：人工评审 pending 时 `review-diff.diff` 存在且内容为基准到任务分支的差异
- [ ] `docs/data-model.md` 任务目录清单与 `docs/operations.md` §（决策 124）与实现一致
