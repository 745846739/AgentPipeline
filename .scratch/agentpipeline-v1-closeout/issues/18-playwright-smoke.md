# 18: playwright 双冒烟接线执行

**What to build:** 前端 E2E 层目前是空的——单元与组件层已全绿，但 spec §9 的两条 playwright 用例（① 看板 → 详情 → 页签 → diff 审批合入；② pending → dossier 面板 → resume，断言琥珀面板与顶栏待办计数）从未执行，原因是没有「真 axum 后端 + FakeAgent 临时 home」的 harness。本票落地 harness 并真正跑通两条（仅 Chromium，决策 144）。依赖票 02 提供可回读端口的库内启动入口。

**Blocked by:** 02（serve 沉入 lib + 端口 0 回读）

**Status:** done

- [x] harness 就位：临时 home + FakeAgent + 真实后端，端口经回读获取，用例结束干净回收
- [x] 用例 ① 跑通：看板 → 任务详情 → 页签切换 → diff 审批合入
- [x] 用例 ② 跑通：pending 触发 dossier 面板 → resume，断言琥珀面板与顶栏待办计数
- [x] 纳入可重复执行的门（justfile 目标），只跑 Chromium
- [x] `docs/testing.md` §9「playwright 两条 E2E 尚未执行」更新为已执行，并记用例路径
