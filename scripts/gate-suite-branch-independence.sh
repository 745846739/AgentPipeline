#!/usr/bin/env bash
#
# 闸门套件的**分支无关性**守卫（不变量见 docs/testing.md §8）。
#
# 为什么必须有：`cargo test --workspace` 既是 develop / merge 阶段的**代码闸门**，也是 CI
# 判据，因此它必须在 main 上绿、在**任何任务分支**上绿、在空分支上绿。而「按分支取数」的
# 断言（`{merge-base}...HEAD` 的 diff 形状、逐文件增删行数、改动面白名单）描述的是**任务
# 分支的那一次 diff**——在别的分支上读到的就是那个分支的 diff，必然假红。这类判据一旦溜进
# 常驻套件，就会把**每一个**任务卡在 develop 闸门上。
#
# 实证：2026-10-07 dogfood，任务 01M4A35GGJ53YDJRZ0R3GZTM06 的 develop 闸门连红 4 轮
# （ux_audit3_landing 的 scene_07/11/17 按 `origin/main...HEAD` 取数），任务最终人工取消。
# 更早还有两次同因（82dfdf1、00bafec），每次都只是补一个「同点就跳过」的守卫——都是逐案
# 打补丁。这条脚本把那个教训升格为机器门：**不靠人 review 撞上，靠门拦下**（与 testing.md
# §9 的静态扫描守卫同一姿态）。
#
# 判据（只扫常驻用例 = 不带 `#[ignore]` 的 `#[test]` 函数体）：
#   出现 `..HEAD` 区间（`origin/main...HEAD`、`{mb}..HEAD` 等）或与 `diff` 同行的 `"HEAD"`
#   → 红。命中则给出「怎么改」的指向；**没有任何例外口子**——真要做一次性落地验收，把它
#   写成 `#[ignore]` 的独立用例，读数留档到对应台账。
#
# 范围说明：只扫 `tests/e2e/tests/integration/*.rs` 里 `#[test]` / `#[tokio::test]` 的函数体
# （模块级辅助函数不计——`assert_frozen_untouched` 那类读的是**工作区**未提交改动，在 main /
# 任务分支 / 空分支上都成立，属分支无关）。临时仓驱动的用例（harness 自建 repo）不在同一
# 集合里，故不会误伤。

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET_DIR="tests/e2e/tests/integration"

if [ ! -d "$TARGET_DIR" ]; then
  echo "闸门套件分支无关性：找不到 $TARGET_DIR（脚本位置漂移了？）" >&2
  exit 1
fi

violations=0
while IFS= read -r f; do
  [ -e "$f" ] || continue
  hits=$(awk '
    /^#\[[a-z:]*test\]/ { in_test = 1; ignored = 0; next }
    in_test && /^#\[ignore/ { ignored = 1 }
    in_test && /^}/ { in_test = 0; next }
    # 注释行不算（`//` / `///` 里常写「原来这里按 ..HEAD 取数」这类说明）；
    # 只拦**代码**里的 git 调用。
    in_test && !ignored && $0 !~ /^[[:space:]]*\/\// \
      && ($0 ~ /\.\.HEAD/ || ($0 ~ /diff/ && $0 ~ /"HEAD"/)) {
      printf "    %s:%d: %s\n", FILENAME, FNR, $0
      found = 1
    }
    END { if (found) exit 1 }
  ' "$f") || {
    echo "  ✗ $f 的常驻用例按 HEAD 取数（应改为 #[ignore] 的一次性落地验收）："
    echo "$hits"
    violations=$((violations + 1))
  }
done < <(find "$TARGET_DIR" -maxdepth 1 -name '*.rs' | sort)

if [ "$violations" -gt 0 ]; then
  cat >&2 <<'EOF'

闸门套件分支无关性：红了。

  `cargo test --workspace` 必须在 main / 任何任务分支 / 空分支上都绿（docs/testing.md §8）。
  上面命中的断言读的是**当前分支相对基准的 diff**——在别的分支上必然假红，会把每一个任务
  卡在 develop 闸门。

怎么改：
  - 一次性**落地验收**（「那次落地改了哪几行」）：写成独立的 `#[ignore]` 用例，
    读数留档到该轮的台账（`.scratch/<轮次>/IMPLEMENTATION.md` 一类）。参照
    `ux_audit3_landing.rs::landing_shape_readings_once`。
  - 只要**持久事实**的判据：改读文件内容 / 行号、`git status --porcelain`、或
    `git cat-file -e origin/main:<path>` 这类「基准树里有没有」的判据。
EOF
  exit 1
fi

echo "闸门套件分支无关性：通过（$TARGET_DIR 的常驻用例没有按 HEAD 取数的 git 调用）"
