# 08: architect 重入反馈注入

**What to build:** 两个同落点的重入反馈来源一次补齐——① develop / test 的 retry_exhausted 选「带失败摘要回架构设计修订」时，系统在与游标置位**同一事务**内把重试历史摘要（各次 attempt 的失败原因 + validate_output blockers）写入任务目录 `retry-feedback.md`（决策 138）；② info_insufficient 的补充输入不只落流转原因，还要注入 validate_input 重入的 prompt（决策 79）。architect 重入时 user prompt 追加两个反馈文件的内容（与既有的 `backtrack-feedback.md` 同构），首轮为空不渲染。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] retry_exhausted 回架构设计时写入 `retry-feedback.md`，且与游标置位同事务（不落半截状态）
- [ ] architect 重入的 user prompt 追加 `backtrack-feedback.md` + `retry-feedback.md` 内容，注明来源与诉求（决策 126 / 138）
- [ ] 两文件在首轮执行时为空且不渲染该段
- [ ] info_insufficient 的补充输入注入 validate_input 重入 prompt，而非只落流转原因
- [ ] E2E-21 断言补充输入进入重入 prompt；retry 回架构设计的场景断言 `retry-feedback.md` 落盘与注入
- [ ] `docs/pipeline-spec.md` 打回反馈文件表与实现一致
