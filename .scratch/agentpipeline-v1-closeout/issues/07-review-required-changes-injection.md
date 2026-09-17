# 07: develop 重入注入 review 必须修改项

**What to build:** 流水线 spec 规定 review 打回循环中「每次循环 develop.execute 的 prompt 追加 review 的必须修改项」，但 `required_changes` 目前只随评审报告落库、从不回读，develop 重入时看不到要改什么，只能靠 agent 自己重读报告猜。补上注入：review 打回后 develop.execute 重入的 user prompt 含本轮必须修改项；首轮执行不渲染该段。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] review 判定不通过后，develop.execute 重入的 user prompt 追加必须修改项（含来源标注）
- [x] 首次进入 develop（无上游打回）时不渲染该段
- [x] 修改项取自评审产出而非重新推断；缺失时显式降级为「本次无结构化修改项」而非静默空段
- [x] E2E-03 断言重入 prompt 含修改项内容
