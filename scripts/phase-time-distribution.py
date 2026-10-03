#!/usr/bin/env python3
"""三段分布离线聚合（票 runner-offload/01 验收②）。

给定一个任务的 JSON 日志流，算各阶段「模型时间 / 工具时间 / 无日志间隔」——
只翻日志，不查 DB（B3 的教训：巡检的人不该被要求知道还有一份台账）。

用法:
    python3 scripts/phase-time-distribution.py < 日志.jsonl

输入: LogFormat::Json 的日志行（crates/app/src/serve.rs 的 JsonEvent），至少含
「模型请求派发 / 模型请求收场」（agent/recording.rs，按 request id 配对）与
「工具调用收场」（agent/tools.rs，自带 duration_ms）。

输出: 每个 stage 一份汇总（模型请求数 / 模型时间秒 / 工具调用数 / 工具时间秒），
加全局的 >120s 无日志间隔清单（时间戳秒为粒度；间隔判据对标 2026-10-02 归因
口径——7 分钟级的静默段一眼可见）。
"""
import json
import sys
from datetime import datetime, timezone

GAP_THRESHOLD_S = 120


def to_epoch_s(ts: str) -> float:
    """JsonEvent 的 timestamp（chrono Utc::now().to_rfc3339()，恒带 +00:00）转秒。"""
    dt = datetime.fromisoformat(ts)
    assert dt.tzinfo is not None, f"日志时间戳不带时区: {ts}"
    return dt.timestamp()


def main() -> None:
    events = []
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue  # 混入的非 JSON 行（启动横幅等）跳过
        if e.get("timestamp") and e.get("message"):
            e["ts"] = to_epoch_s(e["timestamp"])
            events.append(e)
    events.sort(key=lambda e: e["ts"])
    stages = sorted({e.get("stage", "-") for e in events})

    model = []
    for s in stages:
        in_stage = [e for e in events
                    if e["message"] == "模型请求收场" and e.get("stage", "-") == s]
        starts = {e["request"]: e["ts"] for e in events
                  if e["message"] == "模型请求派发" and e.get("stage", "-") == s}
        spans = [e["ts"] - starts[e["request"]]
                 for e in in_stage if e.get("request") in starts]
        model.append({
            "stage": s,
            "请求数": len([e for e in events
                           if e["message"] == "模型请求派发" and e.get("stage", "-") == s]),
            "模型时间秒": round(sum(spans)),
        })

    tools = []
    for s in stages:
        closes = [e for e in events
                  if e["message"] == "工具调用收场" and e.get("stage", "-") == s]
        tools.append({
            "stage": s,
            "调用数": len(closes),
            "工具时间秒": round(sum(e.get("duration_ms", 0) for e in closes) / 1000),
        })

    gaps = [b["ts"] - a["ts"] for a, b in zip(events, events[1:])]
    big = [g for g in gaps if g > GAP_THRESHOLD_S]
    print(json.dumps({
        "模型": model,
        "工具": tools,
        "无日志间隔": {
            "阈值秒": GAP_THRESHOLD_S,
            "段数": len(big),
            "合计秒": round(sum(big)),
        },
    }, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
