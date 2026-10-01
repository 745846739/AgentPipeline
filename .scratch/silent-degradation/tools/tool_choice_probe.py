#!/usr/bin/env python3
"""票 07 的实验探针：`tool_choice` 能不能把「文本形态工具调用」的占比压下去。

背景（`.scratch/silent-degradation/issues/07-tool-choice-experiment.md`）：本模型的工具调用
有两种落法——**原生** `tool_calls`（参数完整）与**文本** `<tool_call><function=…>` 落在
`content` 里（此时网关另给一份结构化 `tool_calls`，而那一份会被腰斩）。事故里见过的失败
形态全是第二种。生产请求体不发 `tool_choice`；这个脚本对**同一份 prompt + 工具表**分别按
三档各发 N 次，数 `content` 里出现 `<tool_call>` 的比例：

    baseline（不发 tool_choice） / auto / required

**这是实验，不是修复的前提**：判「有效 / 无效」都不影响票 01 的客户端防御落地。
脚本**只读** DB（取 provider 配置），不改任何产品代码，也不写任何表。

用法：

    # 先小样本（pilot），确认脚本与网关都通
    python3 tool_choice_probe.py --n 3

    # 正式一轮（票面要求 N ≥ 20）
    python3 tool_choice_probe.py --n 20 --prompt-file /tmp/real-prompt.txt

    # 临时覆盖 provider（不读 DB）
    python3 tool_choice_probe.py --base-url https://…/v1/ --api-key sk-… --model x/y --n 20

与生产的差异（读结论时要知道）：
  * 走**非流式**（`stream: false`）——生产是流式。流式与非流式在网关侧是两条路径，
    「文本形态」是模型/网关的行为，理论上与是否流式无关；但若三档都见不到 `<tool_call>`，
    先别下"无效"的结论，改成流式复核一次（`--stream`）。
  * 工具表是**精简子集**（名字与参数形状与生产一致，描述从简）。用 `--prompt-file` 喂真实
    prompt 可以让输入更接近现场。
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

# 与生产同名同形状的工具子集（`crates/core/src/agent/catalog.rs` 的 8 个内置工具里挑 5 个，
# 外加 submit_metadata——文本形态的现场就是它）。
TOOLS = [
    {"type": "function", "function": {
        "name": "read_file",
        "description": "读取文件内容。相对路径按工作目录解析。",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string"},
            "offset": {"type": "integer"},
            "limit": {"type": "integer"},
            "tail": {"type": "boolean"},
        }, "required": ["path"]}}},
    {"type": "function", "function": {
        "name": "write_file",
        "description": "写入文件（覆盖）。",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string"}, "content": {"type": "string"},
        }, "required": ["path", "content"]}}},
    {"type": "function", "function": {
        "name": "list_dir",
        "description": "列出目录内容。",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string"}, "recursive": {"type": "boolean"},
        }, "required": []}}},
    {"type": "function", "function": {
        "name": "run_command",
        "description": "执行一条命令并返回输出。",
        "parameters": {"type": "object", "properties": {
            "command": {"type": "string"}, "timeout_sec": {"type": "integer"},
        }, "required": ["command"]}}},
    {"type": "function", "function": {
        "name": "submit_metadata",
        "description": "提交本节点的结构化元数据（最后一步必须调用它）。",
        "parameters": {"type": "object", "properties": {
            "readiness": {"type": "boolean"},
            "blockers": {"type": "array", "items": {"type": "string"}},
        }, "required": ["readiness"]}}},
]

DEFAULT_SYSTEM = (
    "你是架构设计 agent。根据用户需求生成设计文档：先写 design.md，"
    "最后调用 submit_metadata 返回元数据（readiness / blockers）。"
)
DEFAULT_USER = "任务标题：修复前端闪屏问题\n任务描述：（空）"


def provider_from_db(db: str, provider_id: str | None):
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    if provider_id:
        row = conn.execute("SELECT * FROM providers WHERE id = ?", (provider_id,)).fetchone()
    else:
        row = conn.execute(
            "SELECT * FROM providers WHERE enabled = 1 ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
    if not row:
        raise SystemExit("DB 里没有可用的 provider（用 --base-url/--api-key/--model 覆盖）")
    return row["base_url"], row["api_key"], row["model"]


def transcript_from_db(db: str, conversation_id: int):
    """拿一次真实调用的 system + user + 转录（`kanban_node_conversations`）。

    `messages_json` 里就是 assistant / tool 往来（system 与 user 是独立的列），故直接接在
    两个首条之后即可，不会重复。
    """
    conn = sqlite3.connect(db)
    row = conn.execute(
        "SELECT system_prompt, user_prompt, messages_json FROM kanban_node_conversations WHERE id = ?",
        (conversation_id,),
    ).fetchone()
    if not row:
        raise SystemExit(f"会话 {conversation_id} 不存在")
    system, user, raw = row
    messages = json.loads(raw) if raw else []
    return system or DEFAULT_SYSTEM, user or DEFAULT_USER, messages


def one_call(base_url: str, api_key: str, model: str, system: str, user: str,
             transcript: list, tool_choice: str | None, timeout: float) -> dict:
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            *transcript,
        ],
        "tools": TOOLS,
        "stream": False,
    }
    if tool_choice is not None:
        body["tool_choice"] = tool_choice
    data = json.dumps(body).encode()
    req = urllib.request.Request(
        base_url.rstrip("/") + "/chat/completions",
        data=data,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {api_key}"},
    )
    # 空 User-Agent：**与生产同形**。生产的 reqwest 不发这个头，而 urllib 默认发
    # `Python-urllib/3.x`——实测该网关前面的 Cloudflare 会据此回 403 / error code 1010。
    # 这里不是在绕什么检查，是让探针的请求长得和产品发出去的那一个一样（否则量到的
    # 全是网关的拒绝，不是模型的行为）。
    req.add_header("User-Agent", "")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            payload = json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        return {"ok": False, "error": f"HTTP {e.code}: {e.read().decode()[:300]}"}
    except Exception as e:  # noqa: BLE001 —— 探针要如实记下任何失败形态
        return {"ok": False, "error": f"{type(e).__name__}: {e}"}

    choice = (payload.get("choices") or [{}])[0]
    message = choice.get("message") or {}
    content = message.get("content") or ""
    native = message.get("tool_calls") or []
    return {
        "ok": True,
        "text_form": "<tool_call>" in content,
        "native_calls": len(native),
        "finish_reason": choice.get("finish_reason"),
        "content_head": content[:200],
    }


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--db", default="~/.agentpipeline/data/agentpipeline.db")
    ap.add_argument("--provider-id")
    ap.add_argument("--base-url")
    ap.add_argument("--api-key")
    ap.add_argument("--model")
    ap.add_argument("--n", type=int, default=3, help="每组次数（票面要求 ≥ 20）")
    ap.add_argument("--groups", default="baseline,auto,required")
    ap.add_argument("--prompt-file", help="用真实 prompt 替换默认 user 段")
    ap.add_argument("--conversation-id", type=int,
                    help="从 DB 取这一次真实调用的 system + user + 转录（最贴近现场）")
    ap.add_argument("--timeout", type=float, default=120.0)
    ap.add_argument("--json", help="把原始计数写到这个文件")
    args = ap.parse_args()

    import os
    db = os.path.expanduser(args.db)
    base_url, api_key, model = (args.base_url, args.api_key, args.model)
    if not (base_url and api_key and model):
        db_url, db_key, db_model = provider_from_db(db, args.provider_id)
        base_url = base_url or db_url
        api_key = api_key or db_key
        model = model or db_model

    system = DEFAULT_SYSTEM
    user = DEFAULT_USER
    transcript: list = []
    if args.conversation_id:
        system, user, transcript = transcript_from_db(db, args.conversation_id)
    elif args.prompt_file:
        with open(os.path.expanduser(args.prompt_file), encoding="utf-8") as f:
            user = f.read()

    groups = [g.strip() for g in args.groups.split(",") if g.strip()]
    choice_of = {"baseline": None, "auto": "auto", "required": "required"}
    print(f"model={model} base_url={base_url} n={args.n} groups={groups}")
    print(f"prompt 长度：system {len(system)} / user {len(user)} 字符，转录 {len(transcript)} 条\n")

    counts = {}
    for group in groups:
        if group not in choice_of:
            raise SystemExit(f"未知分组：{group}（可选 baseline / auto / required）")
        text_form = native = failed = 0
        samples = []
        for i in range(args.n):
            r = one_call(base_url, api_key, model, system, user, transcript,
                         choice_of[group], args.timeout)
            if not r["ok"]:
                failed += 1
                samples.append(f"  [{i+1}] 失败：{r['error']}")
                continue
            if r["text_form"]:
                text_form += 1
            if r["native_calls"]:
                native += 1
            samples.append(
                f"  [{i+1}] text_form={r['text_form']} native={r['native_calls']} "
                f"finish={r['finish_reason']}"
            )
        counts[group] = {
            "n": args.n, "text_form": text_form, "native_call_responses": native,
            "failed": failed,
        }
        print(f"== {group} ==")
        print("\n".join(samples))
        print(f"   文本形态 {text_form}/{args.n}（失败 {failed}）\n")

    print("=== 汇总 ===")
    for group, c in counts.items():
        ratio = c["text_form"] / c["n"] if c["n"] else 0
        print(f"{group:9s} 文本形态 {c['text_form']}/{c['n']} = {ratio:.0%}，"
              f"带原生 tool_calls {c['native_call_responses']}/{c['n']}，失败 {c['failed']}")

    if args.json:
        out = {
            "measured_at": datetime.now(timezone.utc).isoformat(),
            "model": model, "base_url": base_url, "counts": counts,
        }
        with open(os.path.expanduser(args.json), "w", encoding="utf-8") as f:
            json.dump(out, f, ensure_ascii=False, indent=2)
        print(f"原始计数已写入 {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
