#!/usr/bin/env python3
r"""措辞校正第二轮（两轴评审收口）：补掉第一轮漏网的 8 处「行」参照。

第一轮的匹配模式（本行 / 该行 / 各行 / 行内 / 表行 / 落表 / 见 N 行…）漏掉了
几种形态：被修订的行、对应行、N 行保留占位、那行、N 行不动、原行不动、
N 行原文保留（210 / 293）。本轮逐处替换并输出 wording-manifest-2.md。

刻意保留、不入本清单的「行」（第二遍穷尽扫描后的豁免全集）：
- 数据库 / 台账的行：那行 pending 的 continue、失败账行、别项目一行不动、
  `raise_provider_context_window` 把该行只上调、伴随行在原行之后；
- 界面 / 版面的行：折叠行、坞那行提示语、两行结构、六行像素字、动作行折两行…；
- 代码 / 日志行号：逐行列出（行号 + 原行文本）、与原行为逐字等价、~700 行内联测试…；
- 习语（不指表格行）：信任标记一行不动、政策面一行不动、`tool_timeout_sec` 一行不动；
- 其他仍按行组织的文档的行：glossary 两行就地修订、frontend-design §12.3 那一行、
  overview.md §3 参数表两行、testing.md 第 286 行 / §3.1 第 5 行 / §10 的 255 行 /
  §12.3 第 780 行、票 16 第 14 行；
- 「274 的边界原文保留」这类不带「行」字的表述，本身不涉参照系。
"""

PAIRS: list[tuple[str, str, int]] = [
    ("**被修订的行保留原文不删**，修订点与代价见对应行的正文",
     "**被修订的条目保留原文不删**，修订点与代价见对应条目的正文", 1),
    ("决策 106 并入 101（106 行保留占位）", "决策 106 并入 101（106 条保留占位）", 1),
    ("执行决策 245⑤**：那行「不删", "执行决策 245⑤**：那条「不删", 1),
    ("**修订 176① 的现时口径**（176 行不动）", "**修订 176① 的现时口径**（176 条不动）", 1),
    ("（176/207 原行不动、冲突按本条编号标注", "（176/207 原条目不动、冲突按本条编号标注", 1),
    ("**修订 176④/207③ 的现时口径**（两行原文不动）",
     "**修订 176④/207③ 的现时口径**（两条原文不动）", 1),
    ("**显式修订决策 210⑧ 的判据面**（210 行原文保留）",
     "**显式修订决策 210⑧ 的判据面**（210 条原文保留）", 1),
    ("**显式修订决策 293**（293 行原文保留，判据 ①② 与两个常量不动）",
     "**显式修订决策 293**（293 条原文保留，判据 ①② 与两个常量不动）", 1),
]


def main() -> None:
    from pathlib import Path
    src = Path("docs/decisions.md")
    text = src.read_text(encoding="utf-8")
    manifest: list[str] = ["# 措辞校正清单·第二轮（两轴评审收口）", ""]
    bad: list[str] = []
    for old, new, want in PAIRS:
        got = text.count(old)
        if got != want:
            bad.append(f"预期 {want} 次、实得 {got} 次：{old[:50]}…")
            continue
        text = text.replace(old, new)
        manifest.append(f"- 「{old}」→「{new}」×{want}")
    if bad:
        raise SystemExit("FAIL:\n" + "\n".join(bad))
    src.write_text(text, encoding="utf-8")

    domain = Path("docs/agents/domain.md")
    dtext = domain.read_text(encoding="utf-8")
    dold, dnew = "被修订时在行内标注", "被修订时在条目内标注"
    assert dtext.count(dold) == 1, "domain.md 引用措辞未命中"
    domain.write_text(dtext.replace(dold, dnew), encoding="utf-8")
    manifest.append(f"- docs/agents/domain.md：「{dold}」→「{dnew}」×1")

    out = Path(__file__).resolve().parent.parent / "wording-manifest-2.md"
    out.write_text("\n".join(manifest) + "\n", encoding="utf-8")
    print(f"OK：{len(PAIRS) + 1} 处替换完成，清单落 {out.name}")


if __name__ == "__main__":
    main()
