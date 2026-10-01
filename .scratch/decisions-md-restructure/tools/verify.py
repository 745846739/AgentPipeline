#!/usr/bin/env python3
r"""decisions.md 重构的三级校验（两轴评审收口后重写）。

链路（每一级都要逐字节相等，任何一级失败即 FAIL）：

1. **结构级**：基线（git 历史里的表格版，默认 `eebaaf3:docs/decisions.md`）
   经 convert.parse + convert.render 重建 == 提交 38a0ed3 的文件
   （证明结构转换提交逐字节正确、无夹带改动）；
2. **措辞级**：对上一级结果依次施加 reword.PAIRS、reword2.PAIRS == 工作区
   当前文件（证明两轮措辞替换之外没有任何其他改动）；
3. **形状级**：364 条一节一条不少、编号集合 = 1–365 去 318、每节 heading
   与基线标题一致（去掉 `**`）、新版无残留表格行。

输出写入 verify-report.txt（随票归档）。基线提交可用 --baseline 覆盖
（默认 eebaaf3；结构提交可用 --structure 覆盖，默认 38a0ed3）。
"""
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from convert import parse, render  # noqa: E402  复用同一套解析，避免逻辑漂移
import reword  # noqa: E402
import reword2  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
NEW = REPO / "docs" / "decisions.md"
REPORT = Path(__file__).resolve().parent.parent / "verify-report.txt"


def git_show(ref: str) -> str:
    return subprocess.run(["git", "show", ref], check=True, cwd=REPO,
                          capture_output=True, text=True).stdout


def apply_pairs(text: str, pairs) -> str:
    for old, new, want in pairs:
        assert text.count(old) == want, f"清单命中数不符（{text.count(old)} != {want}）：{old[:40]}"
        text = text.replace(old, new)
    return text


def main() -> None:
    baseline_ref = "eebaaf3:docs/decisions.md"
    structure_ref = "38a0ed3:docs/decisions.md"
    args = sys.argv[1:]
    for flag, setter in (("--baseline", "baseline"), ("--structure", "structure")):
        if flag in args:
            val = args[args.index(flag) + 1]
            if setter == "baseline":
                baseline_ref = val
            else:
                structure_ref = val

    report: list[str] = []
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        report.append(("PASS  " if cond else "FAIL  ") + msg)
        if not cond:
            ok = False

    prefix_lines, b_cells = parse(git_show(baseline_ref))
    check(len(b_cells) == 364, f"基线解析出 {len(b_cells)} 条决策（预期 364）")

    rebuilt = render(prefix_lines, b_cells)
    check(rebuilt == git_show(structure_ref), f"结构级：基线重建 == {structure_ref}（逐字节）")

    worded = apply_pairs(rebuilt, reword.PAIRS + reword2.PAIRS)
    current = NEW.read_text(encoding="utf-8")
    check(worded == current, "措辞级：结构版 + 两轮清单 == 工作区文件（逐字节）")

    pieces = re.split(r"\n### 决策 (\d+) · ", current)
    nums = [int(pieces[i]) for i in range(1, len(pieces), 2)]
    check(len(nums) == 364, f"形状级：共 {len(nums)} 节（预期 364）")
    check(set(nums) == set(range(1, 366)) - {318}, "形状级：编号集合 = 1–365 去 318")
    # 标题若含「行」参照，措辞替换同样应命中标题，故期望值先过同一套替换
    def word_title(t: str) -> str:
        for old, new, _ in reword.PAIRS + reword2.PAIRS:
            t = t.replace(old, new)
        return t

    bad_heads = [c["num"] for c in b_cells
                 if not current.split(f"\n### 决策 {c['num']} · ", 1)[1]
                 .startswith(word_title(c["title"]).replace("**", "").strip() + "\n")]
    check(not bad_heads, f"形状级：每节 heading 与基线标题一致（异常 {bad_heads[:5]}）")
    leftover = [ln for ln in current.split("\n") if ln.startswith("| ")]
    check(not leftover, f"形状级：无残留表格行（命中 {len(leftover)}）")

    REPORT.write_text("\n".join(report) + "\n", encoding="utf-8")
    print("\n".join(report))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
