#!/usr/bin/env bash
#
# 编译缓存清理（幂等，可随时重跑）。入口是 `make sweep`。
#
# 为什么需要它：本机的构建产物会**只增不减**地堆到十几 GB，而这个堆积恰好是
# `cargo-sweep` 看不见的——后者按**访问时间**判「过时」，可这里的垃圾按时间算全是
# 「新」的。2026-09-18 实测：`cargo sweep --stamp --dry-run` 报 **0**（`--time 4`
# 只有 61 MiB），而项目当时实占 **29 GB**，其中 18 GB 是可再生的缓存与孤儿产物。
#
# 删三类，每一类都实测过「删完 cargo 仍是 0 编译的空转」：
#
#   ① `*/target/{debug,release}/incremental/` —— 增量编译缓存。
#      删掉不会让任何 crate 变脏：实测删掉 root 的 9.2 GB 后，`cargo build --workspace`
#      与 `cargo test --workspace --no-run` 都仍是 0 个 crate 编译（1.9s / 1.7s 空转）。
#      代价只有「下一次改动的首编退化为全编」——决策 178 实测 18s → 3m33s，之后照旧。
#
#   ② `*/target/*/deps/*.rcgu.o` —— 增量编译**每次会话**产生的一次性目标文件：
#      文件名里带会话哈希（`app-<metadata>.<session>.<cgu>.rcgu.o`），cargo 从不回收。
#      2026-09-18 实测 root 一个 target 里堆了 **131,924 个（实际占用 7.1 GB）**。
#      它们**不是链接输入**：实测删净后 build / test 仍 0 编译。
#      **必须用 `find -delete`，不能用 shell 通配符**：131k 个文件交给 `*.o` 会当场撞
#      `ARG_MAX`，而 `ls *.o` 的失败是**静默**的（2>/dev/null 一吞，看起来像「没有 .o」，
#      排查时极易被误导——本次诊断就踩过一次）。
#
#   ③ `target/doc` —— `cargo doc` 的 HTML 产物，随时可重生成，且没人会拿它当事实源。
#
# 不删（删了要付整棵依赖树重编，收益为负）：第三方 rlib / rmeta（root 约 1.9 GB）、
# `target/{debug,release}/build/`、在用的测试二进制、`frontend/node_modules`、`.git`。
#
# **体积一律按 `du`（实际占用）报，不按 `stat`（逻辑字节）**：APFS 会压缩这些目标
# 文件，两者能差一个数量级——2026-09-18 实测 `.rcgu.o` 逻辑 735 MB，而删掉它几乎
# 没让 `df` 动（同一批里真正腾出空间的是增量缓存目录）。报逻辑字节会让人以为释放了
# 735 MB，其实没有。故本脚本报的「已释放」= 被删路径的 `du` 之和。
#
# **不做**的事：不清理「改名 / 合并后遗留的孤儿测试二进制」。这类只在测试布局变更时
# 出现一次，写死在脚本里既会腐坏、又有误删同名新 target 的脚枪。2026-09-18 清理决策 218
# 之前那 33 个独立集成测试 target 的残留用的是一次性命令（可复现）：
#
#   # 33 个名字 = 决策 218 合并前 crates/{core,app}/tests 与 tests/e2e 下的独立测试文件
#   for p in executor timeouts crash_recovery happy_path gates reviews conflicts pending \
#            join_and_skip llm_smoke port_stability api_contract market foreman production_llm \
#            env_mode repair process_group egress deps scheduler_tick cursor_lifecycle \
#            foreman_proposals conversation_archive project_analysis_observation \
#            foreman_sessions_migration server_bind pairing repo_live restart_recovery \
#            git_chain smoke lan_bind zz_tmp_review_probe; do
#     find target/debug/deps -maxdepth 1 -type f \( -name "$p-*" -o -name "$p.rcgu.o" \) \
#          ! -name '*.d' ! -name '*.rlib' ! -name '*.rmeta' ! -name '*.dylib' -delete
#   done
#
# 用法：`make sweep` 真删；`DRY=1 make sweep` 只报将删什么，不动盘。

set -euo pipefail

cd "$(dirname "$0")/.."

DRY="${DRY:-0}"

# 桌面壳是独立 workspace（决策 156），两个 target 树都要扫。路径写死而不走
# `cargo metadata`：本目标的前提之一就是「不惊动 cargo」——不带任何工具链假设。
ROOTS=(target crates/desktop/target)

human() { # KB → 人读单位
  awk -v k="$1" 'BEGIN {
    if (k >= 1048576) printf "%.1f GB", k / 1048576
    else if (k >= 1024) printf "%.1f MB", k / 1024
    else printf "%d KB", k
  }'
}

# 按 `find -name <模式>` 统计两个 target 树里的匹配文件：输出「个数 占用KB」。
# 体积走**逐文件 `du -sk` 求和**——`-s` 让每个参数各占一行，把**所有行**的第一列相加
# 即得总计，故不受 find 的 `-exec ... +` 分批影响。
# **不要写成 `du -ck ... | tail -1`**：分批时 `-c` 的 total 只覆盖最后一批，量出来
# 小一个数量级（本次诊断踩过，10 GB 的 deps 被量成 1 GB 级）。
scan() {
  local pattern="$1" count=0 kb=0 n
  for r in "${ROOTS[@]}"; do
    [ -d "$r" ] || continue
    n=$(find "$r" -name "$pattern" -print | wc -l | tr -d ' ')
    [ "$n" -gt 0 ] || continue
    count=$((count + n))
    kb=$((kb + $(find "$r" -name "$pattern" -exec du -sk {} + \
                 | awk '{s+=$1} END {printf "%d", s+0}')))
  done
  printf '%s %s' "$count" "$kb"
}

if [ "$DRY" = "1" ]; then
  echo "[sweep] DRY=1 → 只报将删什么，不动盘"
fi

freed_kb=0

# ① 增量缓存。整个目录删，故体积直接量目录。
for r in "${ROOTS[@]}"; do
  for p in "$r/debug/incremental" "$r/release/incremental"; do
    [ -d "$p" ] || continue
    kb=$(du -sk "$p" | cut -f1)
    echo "[sweep] 增量缓存 ${p}（$(human "$kb")）"
    freed_kb=$((freed_kb + kb))
    if [ "$DRY" != "1" ]; then rm -rf "$p"; fi
  done
done

# ② 增量编译的一次性目标文件（见文件头 ②）。
read -r o_count o_kb <<<"$(scan '*.rcgu.o')"
echo "[sweep] 一次性目标文件 deps/*.rcgu.o：${o_count} 个（$(human "$o_kb")）"
freed_kb=$((freed_kb + o_kb))
if [ "$o_count" -gt 0 ] && [ "$DRY" != "1" ]; then
  for r in "${ROOTS[@]}"; do
    [ -d "$r" ] && find "$r" -name '*.rcgu.o' -delete
  done
fi

# ③ cargo doc 产物。
if [ -d target/doc ]; then
  doc_kb=$(du -sk target/doc | cut -f1)
  echo "[sweep] cargo doc 产物 target/doc（$(human "$doc_kb")）"
  freed_kb=$((freed_kb + doc_kb))
  if [ "$DRY" != "1" ]; then rm -rf target/doc; fi
else
  echo "[sweep] cargo doc 产物：无"
fi

# 报的是「被删路径的占用之和」，不是 `df` 的净变化：同一份数据可能被 APFS 快照 /
# 其他进程影响，而 `df` 会把那些噪音一并算进来。两者量级一致即可。
if [ "$DRY" = "1" ]; then
  echo "[sweep] 将释放约 $(human "$freed_kb")（DRY：一个字节都没动）"
  echo "[sweep] 去掉 DRY=1 即真删"
elif [ "$freed_kb" -eq 0 ]; then
  echo "[sweep] 无可清理项——缓存不在或已被上一次 sweep 清过（本目标幂等）"
else
  echo "[sweep] 已释放 $(human "$freed_kb")"
  # 缓存删了会重新长回来，只是从零开始长得慢——本目标按需重跑即可。
  echo "[sweep] 接下来建议跑 \`cargo build --workspace\` 与 \`cargo test --workspace --no-run\`："
  echo "[sweep] 两者都应仍是 0 个 crate 编译的空转；若不是，说明删到了不该删的东西。"
fi
