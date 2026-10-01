#!/usr/bin/env python3
"""diff 实效验证（票 01 验收项）：重放三条历史修订的「改结论一句话」。

对同一条决策做同一次语义编辑，比较表格版（基线 eebaaf3）与小节版
（当前工作区）的改动行规模——本次重构的根本动机就是让单条修订的 diff
只落在该条小节内，而不是重写一整行 2k–11k 字符的表格行。

只读不改：结果落 diff-replay.txt。
"""
import re
import subprocess
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "diff-replay.txt"
MARK = "（diff 重放验证）"

CASES = [
    (5, "node 内部直接调用 rig，petgraph 只负责调度"),
    (224, "「查台账查到停不下来」这个病要防"),
    (19, "（workdir_bound + deny_paths + realpath + 拒绝写符号链接）"),
]


def changed_lines(orig: str, needle: str) -> tuple[list[int], list[int]]:
    """做替换，返回 (改动行号, 改动行的原始字符数)。"""
    old_lines = orig.split("\n")
    new_text = orig.replace(needle, needle + MARK, 1)
    assert new_text != orig, f"未命中：{needle[:40]}"
    new_lines = new_text.split("\n")
    assert len(old_lines) == len(new_lines)
    idx = [i for i, (a, b) in enumerate(zip(old_lines, new_lines)) if a != b]
    return idx, [len(old_lines[i]) for i in idx]


def section_of(text: str, line_no: int) -> str:
    """改动行归属的小节标题。"""
    m = None
    for m2 in re.finditer(r"^### 决策 (\d+) · ", text, re.M):
        if text[:m2.start()].count("\n") > line_no:
            break
        m = m2
    return m.group(0)[4:] if m else "(未归属)"


def main() -> None:
    baseline = subprocess.run(["git", "show", "eebaaf3:docs/decisions.md"],
                              capture_output=True, text=True, check=True).stdout
    current = Path("docs/decisions.md").read_text(encoding="utf-8")
    rows = ["# diff 实效验证：同一次语义编辑在两版上的改动行规模", "",
            f"编辑内容：在目标句后追加「{MARK}」（三条各自唯一的结论句）。", ""]
    rows.append("| 决策 | 表格版改动行字符数（行号） | 小节版改动行字符数（行号） | 小节版归属 |")
    rows.append("|---|---|---|---|")
    for num, needle in CASES:
        b_idx, b_len = changed_lines(baseline, needle)
        s_idx, s_len = changed_lines(current, needle)
        owner = section_of(current, s_idx[0])
        rows.append(f"| #{num} | {b_len}（L{b_idx[0]+1}） | {s_len}（L{s_idx[0]+1}） | {owner} |")
    rows += ["", "预期：表格版改动的是一整行表格行（整行重写），小节版只改动结论那一段落行，",
             "且改动行落在目标决策自己小节内。"]
    OUT.write_text("\n".join(rows) + "\n", encoding="utf-8")
    print("\n".join(rows))


if __name__ == "__main__":
    main()
