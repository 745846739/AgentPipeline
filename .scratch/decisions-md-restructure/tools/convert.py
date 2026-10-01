#!/usr/bin/env python3
r"""decisions.md 结构转换：一行一条的表格 → 每条决策一个小节。

硬约束（票 01-decisions-md-restructure）：只改结构，不改一个字的决策内容。
- 表格头 / 分隔行 / 行间空行是结构标记，删除；
- 单元格切分按**结构竖线**：未转义且不在反引号代码内的 `|` 才是分隔符
  （\\| 与代码内竖线都是内容；`\|` 原样保留）；
- 四格行（序号/标题/结论/来源）→ `### 决策 N · 标题` + 结论段 + `**来源**：` 行；
- 三格行（序号/标题/正文，正文自带行内 `**来源**：` 标记，如 291–300）
  → 标题 + 正文段（逐字保留，不再外加来源行）；
- 标题进入 heading 时去掉 `**`（heading 本身已加粗），其余正文逐字节保留
  （含换行——跨物理行的行如决策 285，行内换行原样保留）。

解析结果同时落 decisions-cells.json，供 verify.py 做逐字节比对。
"""
import json
import re
import sys
from pathlib import Path

SRC = Path("docs/decisions.md")
OUT_JSON = Path(__file__).resolve().parent.parent / "decisions-cells.json"

HEADER = "| # | 决策 | 结论 | 来源 |"


def fail(msg: str) -> None:
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def split_structural(s: str) -> list[str]:
    """按结构竖线切分：未转义且不在反引号代码内的 `|`。"""
    cells: list[str] = []
    buf: list[str] = []
    in_code = False
    i = 0
    while i < len(s):
        ch = s[i]
        if ch == "\\" and i + 1 < len(s):
            buf.append(ch)
            buf.append(s[i + 1])
            i += 2
            continue
        if ch == "`":
            in_code = not in_code
        elif ch == "|" and not in_code:
            cells.append("".join(buf))
            buf = []
            i += 1
            continue
        buf.append(ch)
        i += 1
    cells.append("".join(buf))
    return cells


def parse(src: str):
    lines = src.split("\n")
    header_idx = next((i for i, ln in enumerate(lines) if ln == HEADER), None)
    if header_idx is None:
        fail(f"找不到表格头 {HEADER!r}")

    blocks: list[list[str]] = []
    cur: list[str] | None = None
    for ln in lines[header_idx + 2:]:  # 跳过表头与分隔行
        if re.match(r"^\| \d+ \|", ln):
            cur = [ln]
            blocks.append(cur)
        elif ln.strip() == "":
            continue  # 行间空行（结构标记）
        elif cur is not None:
            cur.append(ln)  # 跨行续段（如决策 285）
        else:
            fail(f"表格区出现无法归属的内容：{ln[:80]!r}")

    cells = []
    for block in blocks:
        parts = split_structural("\n".join(block))
        if parts[0].strip() != "" or parts[-1].strip() != "":
            fail(f"行首尾不是结构竖线：{block[0][:60]!r}")
        inner = [p.strip() for p in parts[1:-1]]
        num = inner[0]
        if not re.fullmatch(r"\d+", num):
            fail(f"决策号不合法：{num!r}")
        if len(inner) == 4:
            cells.append({"num": int(num), "title": inner[1], "conclusion": inner[2],
                          "source": inner[3], "inline_source": False})
        elif len(inner) == 3:
            cells.append({"num": int(num), "title": inner[1], "conclusion": inner[2],
                          "source": "", "inline_source": True})
        else:
            fail(f"#{num} 结构竖线切出 {len(inner)} 格（只支持 3 或 4 格）")
    return lines[:header_idx], cells


def render(prefix_lines, cells) -> str:
    out = "\n".join(prefix_lines).rstrip("\n") + "\n"
    for c in cells:
        title_clean = c["title"].replace("**", "").strip()
        out += f"\n### 决策 {c['num']} · {title_clean}\n\n"
        out += f"{c['conclusion']}\n"
        if not c["inline_source"]:
            out += f"\n**来源**：{c['source']}\n"
    return out


def main() -> None:
    src = SRC.read_text(encoding="utf-8")
    prefix, cells = parse(src)

    nums = [c["num"] for c in cells]
    dup = sorted({n for n in nums if nums.count(n) > 1})
    if dup:
        fail(f"决策号重复：{dup}")
    missing = sorted(set(range(1, 366)) - set(nums))
    if missing != [318]:
        fail(f"缺失决策号与预期不符（预期仅 318）：{missing}")
    # 注：表体按落卡顺序排列而非编号升序（176/177 让路取号等历史原因），不校验排序。
    three = [c["num"] for c in cells if c["inline_source"]]
    print(f"三格行（正文自带来源标记，共 {len(three)} 条）：{three}")

    OUT_JSON.write_text(json.dumps(cells, ensure_ascii=False, indent=1), encoding="utf-8")
    SRC.write_text(render(prefix, cells), encoding="utf-8")
    print(f"OK：{len(cells)} 条决策已转换，单元格数据落 {OUT_JSON.name}")


if __name__ == "__main__":
    main()
