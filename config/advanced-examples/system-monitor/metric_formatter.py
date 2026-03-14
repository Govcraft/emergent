"""Stateful metric formatter that computes deltas for rate-based metrics.

Receives raw cumulative counters (CPU, disk I/O, network I/O) and
pass-through metrics (memory, load, disk usage), computes per-second
rates where needed, and publishes normalized monitor.metric events.
"""

import asyncio
import json
import time
from emergent import run_handler, create_message

# Previous samples for delta computation
prev = {}


def unwrap_payload(payload):
    """Unwrap exec-source stdout wrapper and parse JSON."""
    if not payload:
        return None
    raw = payload.get("stdout") or payload.get("metric")
    if raw is None:
        return payload  # already unwrapped
    if isinstance(raw, str):
        try:
            return json.loads(raw)
        except (json.JSONDecodeError, ValueError):
            return None
    return payload


def compute_delta(key, current, fields):
    """Compute per-second deltas for cumulative counter fields.

    Returns None on the first sample (need two points for a rate).
    """
    now = time.monotonic()
    if key not in prev:
        prev[key] = {**{f: current[f] for f in fields}, "_t": now}
        return None

    dt = now - prev[key]["_t"]
    if dt <= 0:
        return None

    result = {}
    for f in fields:
        result[f] = (current[f] - prev[key][f]) / dt
    prev[key] = {**{f: current[f] for f in fields}, "_t": now}
    return result


def format_rate(bps):
    if bps >= 1_048_576:
        return f"{bps / 1_048_576:.1f} MB/s"
    if bps >= 1024:
        return f"{bps / 1024:.1f} KB/s"
    return f"{bps:.0f} B/s"


async def process(msg, handler):
    p = unwrap_payload(msg.payload)
    if not p or "metric" not in p:
        return

    metric = p["metric"]
    out = None

    if metric == "cpu":
        delta = compute_delta("cpu", p, ["idle", "total"])
        if delta and delta["total"] != 0:
            pct = (1 - delta["idle"] / delta["total"]) * 100
            out = {"metric": "cpu", "used_pct": round(pct, 1)}

    elif metric == "diskio":
        delta = compute_delta("diskio", p, ["read_sectors", "write_sectors"])
        if delta:
            read_bps = delta["read_sectors"] * 512
            write_bps = delta["write_sectors"] * 512
            out = {
                "metric": "diskio",
                "read_bps": round(read_bps),
                "write_bps": round(write_bps),
                "read_rate": format_rate(read_bps),
                "write_rate": format_rate(write_bps),
                "total_rate": format_rate(read_bps + write_bps),
            }

    elif metric == "net":
        delta = compute_delta("net", p, ["rx_bytes", "tx_bytes"])
        if delta:
            out = {
                "metric": "net",
                "rx_bps": round(delta["rx_bytes"]),
                "tx_bps": round(delta["tx_bytes"]),
                "rx_rate": format_rate(delta["rx_bytes"]),
                "tx_rate": format_rate(delta["tx_bytes"]),
                "total_rate": format_rate(delta["rx_bytes"] + delta["tx_bytes"]),
            }

    else:
        # memory, load, disk — pass through as-is
        out = p

    if out:
        await handler.publish(
            create_message("monitor.metric").caused_by(msg.id).payload(out)
        )


asyncio.run(
    run_handler(
        None,
        ["metric.cpu", "metric.memory", "metric.load",
         "metric.disk", "metric.diskio", "metric.net"],
        process,
    )
)
