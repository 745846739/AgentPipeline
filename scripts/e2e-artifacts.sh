#!/usr/bin/env bash
#
# 产物新鲜度守卫（主流程票 10 / 决策 166）。
#
# 为什么必须有：前端 E2E 的被测对象有两层，任一层陈旧都会让「测试绿」失去意义——
#   ① `target/debug/agent-pipeline` 落后于 Rust 源码；
#   ② 浏览器加载的是**编译期内嵌**的 `frontend/dist`（决策 155），dist 落后于
#      `frontend/src` 时，页面跑的是旧产物——这一层最隐蔽，因为它同时骗过
#      `cargo build`（Rust 没变就不重编）和肉眼。
#
# 做法：前端源码比 dist 新 → 先 `npm run build`；随后一律 `cargo build -p app`
# （cargo 按依赖与 build.rs 的 rerun-if-changed 判定：frontend/dist 的每个文件都在
# 那组 rerun-if-changed 里，故 dist 一变必然重编内嵌资产表）。**不做静默跳过**：
# 每一步都打印判定结果与理由，让「这次跑的是不是当前代码」可见。
#
# 反向验证（票面要求）：改 frontend/src 任一文件而不手动构建 → 本脚本触发
# `npm run build`；改 Rust 源码 → cargo 重编。见票 10「反向验证」记录。

set -euo pipefail

cd "$(dirname "$0")/.."

DIST_INDEX="frontend/dist/index.html"

# 前端源码集合（构建输入）。package-lock 变化意味着依赖可能变了，一并纳入。
FRONTEND_SOURCES=(
  frontend/src
  frontend/index.html
  frontend/svelte.config.js
  frontend/vite.config.ts
  frontend/package.json
  frontend/package-lock.json
)

# ① 前端产物：是否落后于源码？
if [ ! -f "$DIST_INDEX" ]; then
  echo "[freshness] frontend/dist 不存在 → 构建前端产物"
  (cd frontend && npm run build)
elif [ -n "$(find "${FRONTEND_SOURCES[@]}" -newer "$DIST_INDEX" -print -quit 2>/dev/null)" ]; then
  echo "[freshness] 前端源码比 frontend/dist 新 → 重新构建前端产物"
  (cd frontend && npm run build)
else
  echo "[freshness] frontend/dist 已是最新（本次不重建前端）"
fi

# ② 被测二进制：cargo 增量构建。Rust 源码或 frontend/dist 任一变化都会导致重编
#    （后者经 crates/app/build.rs 的 rerun-if-changed 传递），故这里无需自己比对时间。
echo "[freshness] 构建被测二进制 target/debug/agent-pipeline（cargo 增量）"
cargo build -p app
