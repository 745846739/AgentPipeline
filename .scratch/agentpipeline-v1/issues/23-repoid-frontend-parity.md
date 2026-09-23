# 23: RepoId 前后端两份判定收口 + 两个未钉住的 failure kind

**What to build:** 两件同源的事（「文档声称 ≠ 代码事实」的同一种病，架构评审候选 4 拷问时
裁定只记录、另立一票——决策 250 Q2）：

1. **`RepoId` 合法性判定收成一处。** `docs/glossary.md` 技能来源仓词条与决策 187 原话都
   声称「这条判定只有一处实现，界面上不另写正则」，而 `frontend/src/lib/marketRepos.ts` 的
   `validateRepo` / `normalizeRepo` 是一份 27 行的**重新实现**，自带正则
   （`/^[A-Za-z0-9._-]+$/`），且 `normalizeRepo` 刻意放宽了归一（剥 `www.github.com` 前缀——
   后端 `RepoId::parse` 只剥 `github.com` 那族、不认 `www.`）。文件头自己承认不变量只是
   「**前端输出 ⊆ 后端接受集**」。要收口：子集关系必须**被机器钉住**（不是注释里的承诺），
   而不是把前端变成调后端的壳（页面 hostname 判定可能发生在任何 API 调用之前，
   照决策 246 `localPage` / 共享 fixture 的先例走同源断言，或另有更优形状——实现者裁）。
2. **补两个 API 层未钉住的 failure kind。** 决策 194 裁决⑦ 承诺八类失败「互不混淆」，
   当前 `crates/app/tests/integration/market.rs` 只断言了 6/8：缺
   `repo_unreadable`（生产在 `repo.rs:81` / 401·404 分类）与 `digest_mismatch`
   （`repo.rs:83,1090`，git 对象哈希不符 400）的 **kind 断言**——两者生产代码都在、
   core 层有单测，但端点契约层没有用例钉「界面按 kind 分支时这一类拿得到」。

**Blocked by:** 无（决策 250 已落档，本票是它 Q2 的「另立一票」）

**Status:** ready-for-agent

## 一、RepoId 收口

- [ ] 现状取证：列出前端 `validateRepo` 与后端 `RepoId::parse` 的规则逐条对照表
      （哪条只有一边有、哪条两边语义不同——已知 `www.github.com` 归一是前端独有）
- [ ] 裁定形状（候选，按优先序）：
  - a. 共享 fixture 表（照决策 246 的 `tests/fixtures/host_policy_loopback.json` 先例：
    一张输入→期望 JSON，Rust 表测试与 vitest 同一断言方向）
  - b. 后端导出判定（如 `GET` 端点或构建期产物）——注意它有 API 调用时机问题
  - c. 若维持两份实现：把「子集不变量」从注释升成**会变红的测试**
- [ ] 收口后更新 `glossary.md` 技能来源仓词条与 `marketRepos.ts` 文件头注释——
      两处现在一处说「只有一处」、一处说「子集」，改完必须同口径

## 二、补两个 failure kind 的契约断言

- [ ] `market.rs` 加 `repo_unreadable` 的 kind 断言（构造 401·404 形态的远端）
- [ ] `market.rs` 加 `digest_mismatch` 的 kind 断言（构造对象哈希不符——离线 smart HTTP
      fixture 能不能演「坏对象」先取证，演不了就在 core 层补、契约层记录理由）
- [ ] 八类 → 8/8 的覆盖在票面勾选处写清每一类的钉在哪层

## 验收

- [ ] `make check` 绿（决策 168 的唯一权威闸门）
- [ ] `grep -n "子集\|另写正则" docs/glossary.md frontend/src/lib/marketRepos.ts`
      两处口径一致，且有测试护住
- [ ] 八类 failure kind 各有一条断言（API 层或票面写明的豁免理由）

## 来源

- 架构评审（`improve-codebase-architecture`，2026-09-22）候选 4 拷问 Q2：
  「同病并入 / 只记录不动 / 明确排除」→ 用户裁 **只记录不动，另立一票**（顺带记两个
  未钉住的 kind）
- 决策 250（同批落档）第三段「同病另立一票、本批不动」点名本票
- 决策 194 裁决⑦（八类失败互不混淆）、决策 187（判定只有一处的原话）、
  决策 246（跨语言共享 fixture 的先例）
