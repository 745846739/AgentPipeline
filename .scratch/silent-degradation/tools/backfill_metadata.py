#!/usr/bin/env python3
"""一次性回填：把被腰斩的 `kanban_stage_outputs.metadata_json` 从历史会话正文里捞回来。

**这是事故恢复工具，不是产品能力。** 不挂任何产品入口——产品里挂一个"重建元数据"的按钮，
等于给"元数据不可信"发一张永久通行证。见
`.scratch/silent-degradation/issues/04-sync-gate-fail-closed.md` 的「附：回填脚本」。

为什么捞得回来：本模型的工具调用有两种落法——原生 `tool_calls`（那份被网关腰斩了）
与文本 `<tool_call><function=submit_metadata><parameter=…>` 落在 `content` 里（完整）。
本脚本读的是后者，参数解析口径与产品侧的
`crates/core/src/agent/metadata.rs::find_xml_tool_call` 逐条对齐（先试 JSON，再退回字符串）。

已知边界（2026-10-01 事故）：
  * architect-design：完整 XML 在会话里，能捞回（八个字段齐全）。
  * test-design：完整 XML 在会话里，能捞回（含完整 test_scenarios）。
  * develop-design：会话里只有散文、没有 XML，**捞不回来**——脚本对它只会报告"没找到"。

用法：
    # 先看会改什么（默认 dry-run，不写库）
    python3 backfill_metadata.py --db ~/.agentpipeline/data/agentpipeline.db --task-id <ID>

    # 确认无误再落库
    python3 backfill_metadata.py --db ~/.agentpipeline/data/agentpipeline.db --task-id <ID> --apply

只读 `kanban_node_conversations`，只写 `kanban_stage_outputs.metadata_json`（不改 file_path / stale）。
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from datetime import datetime, timezone

# stage → (stage_outputs.output_type, 该行闸门要消费的字段)
TARGETS = {
    "architect-design": ("design_doc", ["acceptance_criteria"]),
    "test-design": ("test_scenarios", ["test_scenarios"]),
}

TOOL = "submit_metadata"


def parse_xml_param_value(raw: str):
    """与产品侧 `parse_xml_param_value` 同口径：去两头空白，JSON 优先，退回字符串。"""
    text = raw.strip()
    try:
        return json.loads(text)
    except ValueError:
        return text


def parse_xml_parameters(params: str) -> dict:
    """`<parameter=K>V</parameter>` 序列 → dict。逐字符扫描，不吃 JSON 里的 `>`。"""
    OPEN, CLOSE = "<parameter=", "</parameter>"
    obj: dict = {}
    cursor = params
    while True:
        p = cursor.find(OPEN)
        if p < 0:
            break
        after_key = cursor[p + len(OPEN):]
        gt = after_key.find(">")
        if gt < 0:
            break
        key = after_key[:gt].strip()
        value_text = after_key[gt + 1:]
        close = value_text.find(CLOSE)
        if close >= 0:
            raw, cursor = value_text[:close], value_text[close + len(CLOSE):]
        else:
            raw, cursor = value_text, ""
        if key:
            obj[key] = parse_xml_param_value(raw)
        if not cursor:
            break
    return obj


def find_xml_tool_call(content: str, tool: str):
    """与产品侧 `find_xml_tool_call` 同口径：找首个非空的 `<function=tool>` 参数块。"""
    marker = f"<function={tool}>"
    rest = content
    while True:
        at = rest.find(marker)
        if at < 0:
            return None
        body = rest[at + len(marker):]
        end = body.find("</function>")
        params = body if end < 0 else body[:end]
        obj = parse_xml_parameters(params)
        if obj:
            return obj
        if end < 0:
            return None
        rest = body[end + len("</function>"):]


def candidates(conn: sqlite3.Connection, task_id: str):
    """按 stage 找出该任务的历史会话里所有可捞的 submit_metadata（按会话 id 升序）。"""
    rows = conn.execute(
        "SELECT id, stage, node, messages_json FROM kanban_node_conversations"
        " WHERE task_id = ? AND stage IN ({}) AND node = 'execute'"
        " ORDER BY id".format(",".join("?" * len(TARGETS))),
        (task_id, *TARGETS.keys()),
    ).fetchall()
    found = []
    for conv_id, stage, _node, messages_json in rows:
        if not messages_json:
            continue
        try:
            messages = json.loads(messages_json)
        except ValueError:
            continue
        for idx, msg in enumerate(messages):
            content = msg.get("content") or ""
            if f"<function={TOOL}>" not in content:
                continue
            value = find_xml_tool_call(content, TOOL)
            if value:
                found.append((stage, conv_id, idx, value))
    return found


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", required=True, help="agentpipeline.db 路径")
    ap.add_argument("--task-id", required=True, help="要回填的看板任务 id")
    ap.add_argument("--apply", action="store_true", help="真的写库（缺省只报告）")
    args = ap.parse_args()

    conn = sqlite3.connect(args.db)
    conn.row_factory = sqlite3.Row

    found = candidates(conn, args.task_id)
    if not found:
        print(f"[miss] 任务 {args.task_id} 的会话里没有可捞的 submit_metadata XML")
        return 1

    now = datetime.now(timezone.utc).isoformat()
    applied = 0
    for stage, conv_id, idx, value in found:
        output_type, consumed = TARGETS[stage]
        missing = [k for k in consumed if k not in value]
        current = conn.execute(
            "SELECT metadata_json FROM kanban_stage_outputs"
            " WHERE task_id = ? AND stage = ? AND output_type = ?",
            (args.task_id, stage, output_type),
        ).fetchone()
        current_keys = sorted(json.loads(current["metadata_json"]).keys()) if current and current["metadata_json"] else None

        print(f"[hit] stage={stage} conv={conv_id} msg#{idx} 字段={sorted(value.keys())}")
        print(f"      当前 metadata_json 的键：{current_keys}")
        print(f"      闸门消费的字段缺不缺：{missing or '齐'}")
        if missing and stage in TARGETS:
            print(f"      ⚠ 捞回的这份仍缺 {missing}，写进去也过不了闸门（票 04）")

        if current is None:
            print("      （该 stage/output_type 还没有行，需要先有产出——脚本不新建行）")
            continue
        if args.apply:
            conn.execute(
                "UPDATE kanban_stage_outputs SET metadata_json = ?, updated_at = ?"
                " WHERE task_id = ? AND stage = ? AND output_type = ?",
                (json.dumps(value, ensure_ascii=False), now, args.task_id, stage, output_type),
            )
            applied += 1
            print("      → 已写库")
        else:
            print("      → dry-run：未写库（加 --apply 才落）")

    if args.apply:
        conn.commit()
        print(f"完成：改动 {applied} 行")
    else:
        print("完成：dry-run，未写库")
    return 0


if __name__ == "__main__":
    sys.exit(main())
