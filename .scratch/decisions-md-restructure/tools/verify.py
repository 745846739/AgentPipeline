#!/usr/bin/env python3
"""decisions.md 结构转换的逐字节校验。

对照基线（git HEAD 的表格版，存为 original-decisions.md）与转换后的新版，断言：
1. 表格前的前缀逐字节一致；
2. 364 条决策一节一条不少，编号集合 = 1–365 去 318，升序；
3. 每节 heading 形状正确（`### 决策 N · 标题`，标题不含 `**`）；
4. 四格行：结论段 + `**来源**：` 行与基线单元格逐字节一致；
   三格行：正文段（含行内来源标记）与基线逐字节一致；
5. 新版不再残留任何表格行形态（行首 `| `）；
6. convert.py 落的 decisions-cells.json 与基线解析结果一致。

输出写入 verify-report.txt（随票归档）。
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from convert import parse  # noqa: E402  复用同一套解析，避免两套逻辑漂移

BASELINE_LOCAL = Path(__file__).resolve().parent.parent / "original-decisions.md"
CELLS = Path(__file__).resolve().parent.parent / "decisions-cells.json"
REPORT = Path(__file__).resolve().parent.parent / "verify-report.txt"
NEW = Path(__file__).resolve().parents[3] / "docs" / "decisions.md"
# 转换前的基线提交（表格版最后一版）；本地副本不入库，缺失时从 git 历史重建
BASELINE_GIT = "eebaaf3:docs/decisions.md"


def baseline_text() -> str:
    if BASELINE_LOCAL.exists():
        return BASELINE_LOCAL.read_text(encoding="utf-8")
    import subprocess
    return subprocess.run(["git", "show", BASELINE_GIT], check=True,
                          capture_output=True, text=True).stdout


def main() -> None:
    report: list[str] = []
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        report.append(("PASS  " if cond else "FAIL  ") + msg)
        if not cond:
            ok = False

    b_prefix_lines, b_cells = parse(baseline_text())
    check(len(b_cells) == 364, f"基线解析出 {len(b_cells)} 条决策（预期 364）")

    new = NEW.read_text(encoding="utf-8")
    pieces = re.split(r"\n### 决策 (\d+) · ", new)
    n_prefix = pieces[0].rstrip("\n") + "\n"
    b_prefix = "\n".join(b_prefix_lines).rstrip("\n") + "\n"
    check(b_prefix == n_prefix, "表格前的前缀逐字节一致")

    nums = [int(pieces[i]) for i in range(1, len(pieces), 2)]
    bodies = dict(zip(nums, (pieces[i] for i in range(2, len(pieces), 2))))
    check(len(nums) == 364, f"新版共 {len(nums)} 节（预期 364）")
    check(set(nums) == set(range(1, 366)) - {318}, "编号集合 = 1–365 去 318")

    if CELLS.exists():
        json_cells = json.loads(CELLS.read_text(encoding="utf-8"))
        check([dict(c) for c in json_cells] == b_cells, "decisions-cells.json 与基线解析一致")
    else:
        report.append("SKIP   decisions-cells.json 不在（中间产物不入库；convert.py 可重建）")

    bad: list[str] = []
    for c in b_cells:
        n = c["num"]
        body = bodies.get(n)
        if body is None:
            bad.append(f"#{n} 缺节")
            continue
        title_clean = c["title"].replace("**", "").strip()
        head = f"{title_clean}\n\n"
        if not body.startswith(head):
            bad.append(f"#{n} heading 不匹配")
            continue
        rest = body[len(head):]
        if c["inline_source"]:
            want = c["conclusion"] + "\n"
        else:
            want = c["conclusion"] + "\n\n**来源**：" + c["source"] + "\n"
        if rest != want:
            bad.append(f"#{n} 正文与基线不一致")
    check(not bad, "逐条 heading / 正文 / 来源比对" + ("" if not bad else f"：{bad[:10]}"))

    leftover = [ln for ln in new.split("\n") if ln.startswith("| ")]
    check(not leftover, f"新版无残留表格行（命中 {len(leftover)}）")

    REPORT.write_text("\n".join(report) + "\n", encoding="utf-8")
    print("\n".join(report))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
