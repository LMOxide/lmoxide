#!/usr/bin/env python3
"""Coverage report for an LMO live capture: what was received, what decoded, what is lost.

Writes data/live-capture/capture-coverage.json (default) and prints a short summary.

Usage:
  python3 tools/re/live_coverage.py CAPTURE [--out PATH]

Sources:
  - livecap.build_directions / flow_messages: TCP reassembly and decode state per flow.
  - ctrack analytics GET requests (37.18.192.73:80): playfield ids seen by the client.
  - NetMon header: capture start time (SYSTEMTIME, local time of the capture PC).
"""
import argparse
import collections
import datetime
import json
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import livecap  # noqa: E402
import netmon  # noqa: E402

DEFAULT_OUT = os.path.join(HERE, "..", "..", "data", "live-capture", "capture-coverage.json")
TICKS_PER_SECOND = 10_000_000  # NetMon record timestamps are 100-ns ticks from capture start


def capture_start(path):
    with open(path, "rb") as fh:
        head = fh.read(24)
    year, month, _dow, day, hour, minute, sec, ms = struct.unpack_from("<8H", head, 8)
    return datetime.datetime(year, month, day, hour, minute, sec, ms * 1000).isoformat(), \
        "local time of the capture PC (NetMon SYSTEMTIME, no zone)"


def ctrack_events(dirs):
    """Analytics GETs to ctrack, with the capture time of each request."""
    events = []
    for key, d in dirs.items():
        if key[2] != "37.18.192.73" or key[3] != 80:
            continue
        for run in d.runs:
            for m in re.finditer(rb"GET /track\?([^ ]+) HTTP", run.data):
                query = m.group(1).decode(errors="replace")
                params = dict(p.split("=", 1) for p in query.split("&") if "=" in p)
                frame_no, ts = run.frame_at(m.start())
                events.append({"t": round(ts / TICKS_PER_SECOND, 3) if ts is not None else None,
                               "frame": frame_no, **params})
    return sorted(events, key=lambda e: (e["t"] is None, e["t"] or 0))


def playfields_from_ctrack(events):
    seen = collections.OrderedDict()
    for e in events:
        v = e.get("value", "")
        m = re.fullmatch(r"(\d+):(-?\d+)", v)
        if e.get("action", "").endswith("splashscreen") and m:
            seen.setdefault(m.group(2), []).append(e.get("action"))
    return {k: sorted(set(v)) for k, v in seen.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("--out", default=DEFAULT_OUT)
    a = ap.parse_args()

    dirs, stats = livecap.build_directions(a.capture)
    tables = livecap.load_method_names()
    flows = {}
    totals = collections.Counter()
    for key, d in sorted(dirs.items()):
        direction, frames, info = livecap.flow_messages(d)
        n_rpc = 0
        for _off, ctl, body, _run, _frame, _ts in frames:
            if not ctl:
                n_rpc += 1
        name = "%s %s:%d" % (direction, key[2] if direction == "C->S" else key[0],
                             key[3] if direction == "C->S" else key[1])
        row = dict(info)
        row["rpc"] = n_rpc
        flows[name] = row
        totals["%s_rpc" % direction.replace("->", "")] += n_rpc
        totals["%s_bytes" % direction.replace("->", "")] += info["bytes"]
        if info.get("mode") == "zlib":
            totals["zlib_streams_broken"] += 1 if info.get("zlib_runs_lost") else 0

    funcom_ips = sorted({k[2] for k in dirs if k[0].startswith("192.168.")} |
                        {k[0] for k in dirs if not k[0].startswith("192.168.")})
    events = ctrack_events(dirs)
    start, start_note = capture_start(a.capture)
    out = {
        "capture": os.path.basename(a.capture),
        "capture_start": start,
        "capture_start_note": start_note,
        "frames_total": stats["frames"],
        "funcom_tcp_frames": stats["funcom_tcp_frames"],
        "funcom_endpoints": funcom_ips,
        "totals": dict(totals),
        "client_to_server": "fully decoded: plain LMO framing, CRC-verified; holes resynced by CRC scan",
        "server_to_client": ("main game stream 37.18.193.16:7112 is zlib-compressed; it breaks at the "
                             "first hole in the capture, so only its first bytes decode. Other "
                             "server streams have holes in the same way."),
        "playfields_from_analytics": playfields_from_ctrack(events),
        "analytics_events": events,
        "flows": flows,
    }
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, "w") as fh:
        json.dump(out, fh, indent=2)
        fh.write("\n")
    print("wrote", os.path.abspath(a.out))
    print("totals", dict(totals))
    print("playfields (analytics)", list(out["playfields_from_analytics"]))


if __name__ == "__main__":
    main()
