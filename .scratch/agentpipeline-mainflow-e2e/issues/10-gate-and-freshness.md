# 10: 纳入闸门 + 产物新鲜度守卫

**What to build:** 前 9 张票的结论是否可信，取决于两件事，而这两件当前都不成立：
① 这条测试不会自动跑；② 跑的可能不是当前代码。

**Blocked by:** 01

**Status:** done

- [x] **产物新鲜度守卫**：跑用例前确保被测二进制与内嵌产物是**当前源码**
- [x] 守卫覆盖票 01 之后的现实：`frontend/dist` 的新旧纳入判定
- [x] **纳入闸门**：`frontend-e2e` 进入 `just default`
- [x] **CI**：本项目**无 CI**，已显式记录
- [x] 更新 `README.md` 质量闸门段
- [x] 更新 `docs/testing.md` §9 / §10
- [x] **反向验证**：改前端源码不重建 → 守卫重建；让一张 e2e 失败 → 闸门真红
- [x] 全量闸门绿

## 交付

### ① 产物新鲜度守卫 — `scripts/e2e-artifacts.sh`

两层被测对象都可能陈旧，**第二层最隐蔽**：浏览器加载的是编译期内嵌的 `frontend/dist`
（决策 155），dist 落后于 `frontend/src` 时页面跑旧产物——这一层同时骗过 `cargo build`
（Rust 没变就不重编）和肉眼。

守卫逻辑：

1. `frontend/dist/index.html` 不存在，或前端源码集合（`frontend/src` / `index.html` /
   `svelte.config.js` / `vite.config.ts` / `package.json` / `package-lock.json`）中任一文件
   比它新 → `npm run build`；
2. 随后一律 `cargo build -p app`（增量）。`frontend/dist` 的每个文件都在
   `crates/app/build.rs` 的 `rerun-if-changed` 里，故 dist 一变必然重编内嵌资产表——
   这一步无需自己比对时间。

**每步打印判定与理由，不做静默跳过**：让「这次跑的是不是当前代码」在输出里可见。

### ② 闸门接入

`justfile`：

```
default: lint test frontend frontend-e2e
```

新增 `just frontend`（`npm test` + `npm run check` + `npm run build`）；
`frontend-e2e` 前置 `bash scripts/e2e-artifacts.sh`。这**扩展了决策 147** 的
`default` 组成（四项子目标语义不变），已在 `docs/decisions.md` 追加**决策 166**。

`Makefile`：`just` 未必装在每台机器上（作者环境即未安装——决策 155 的 Makefile 正因
此存在），故镜像一组 `check` / `check-lint` / `check-test` / `check-frontend` /
`check-e2e`，语义与 justfile 对齐、**以 justfile 为准**。

### ③ 无 CI（显式记录，不留未记录状态）

本仓库**无 CI 配置**（无 `.github/workflows/`，且无 git remote）。闸门靠本地
`just default`（或未装 just 时的 `make check`）。这不是遗漏，是当前形态；
若将来引入 CI，`frontend-e2e` 需要 Node + Chromium，须在配置里装好。

## 反向验证（守卫本身被测过）

票面要求「守卫本身也要被验，否则它可能只是文档里的一句话」——两条都实测：

| 场景 | 操作 | 实测结果 |
|---|---|---|
| 前端产物陈旧 | 向 `frontend/src/components/board/TaskCard.svelte` 追加一行**不手动构建** | 守卫打印 `前端源码比 frontend/dist 新 → 重新构建前端产物` → `vite build ✓ built in 3.14s` → `Compiling app`（dist 变化经 build.rs 传递，二进制跟着重编） |
| e2e 真失败 | 在 `happy-path.spec.ts` 注入一条必然为假的 `toHaveText` 断言 | `make check-e2e` **退出码 2**，`1 failed · 16 passed`（探针已移除，当前 17 passed） |
| 幂等（无误报） | 源码无改动连跑两次 | 第二次 `frontend/dist 已是最新`，cargo `Finished in 1.30s`（不重复重建） |

## 验证

- `bash scripts/e2e-artifacts.sh`：幂等（第二次不重建）+ 陈旧检测（源码更新即重建）。
- `make -n check` 展开出 8 条提交前命令，与 justfile `default` 的 4 个子目标一一对应。
- 全量闸门（见 `README.md` / `docs/testing.md` §10 的当前数字）：`cargo test --workspace`
  **532 passed**、vitest **88 passed**、`svelte-check` 0 error / 0 warning、
  playwright **17 passed**、`fmt` / `clippy -D warnings` 干净。

## 与决策的关系

- **决策 147**：把前端 E2E 升格为提交前必过属于对其闸门组成的扩展 → 已在
  `docs/decisions.md` 追加**决策 166**（只追加，未改既有行）。
- **决策 155**：新鲜度守卫的「必须覆盖内嵌 dist」正是 155 的产物形态带来的前提。
