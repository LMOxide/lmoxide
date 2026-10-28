#!/usr/bin/env python3
"""Decode LMO live-server traffic from a Network Monitor capture (see netmon.py) or a pcap/pcapng
file (pcap.py). A flow's direction is server->client when its source is a Funcom host.

Pipeline:
  1. keep TCP packets to/from Funcom's LMO hosts (FUNCOM_PREFIXES),
  2. reassemble each TCP direction by sequence number. Retransmits are dropped. A hole
     (bytes missing from the capture) starts a new "run",
  3. split each run into LMO frames: u32be len, u32be crc32(body), body, len = 4 + len(body),
     bit 31 of len = control frame,
  4. decode each RPC body: u32be hlen, RpcHeader protobuf (proto/Util.proto), BinaryStream args.

Server->client streams are zlib-compressed after a control frame at the start of the
connection (the zlib stream begins at byte 9 of the first run). A hole in a zlib stream cannot
be recovered, so the rest of that stream is reported as lost. Plain streams resync by
scanning for the next frame whose CRC matches.

Usage:
  python3 tools/re/livecap.py CAPTURE [--census OUT.json] [--jsonl OUT.jsonl]
"""
import argparse
import binascii
import collections
import json
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import netmon  # noqa: E402
import pcap  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
RPC_METHODS = os.path.join(HERE, "..", "..", "data", "rpc_methods.json")

# Funcom hosts in the capture:
#   37.18.192.52   UniverseAgent/Interface (7001); 7100/7101 territory/universe
#   37.18.192.72/78 :443 web, 37.18.192.73 :80 ctrack (funnel)
#   37.18.193.16   game/territory servers (7102-7119)
#   69.16.175.42   update.lego.funcom.com :80 (patch CDN)
# Beta captures (LMO_Packets.zip, 2013-12 to 2014-08) use 37.18.192.52/53 and 37.18.193.41/42/61.
FUNCOM_PREFIXES = ("37.18.192.", "37.18.193.", "69.16.175.")

# Endpoint names from crates/lmoxide-net/src/rpc.rs (Endpoint enum).
ENDPOINT_NAMES = [
    "UniverseAgent", "UniverseInterface", "PlayerAgent", "PlayerInterface",
    "CSPlayerAgent", "CSPlayerInterface", "CSPlayerProjectAgent", "CSPlayerProjectInterface",
    "ClientToGameserverRPC", "GameserverToClientRPC", "GameCharAgent", "GameCharInterface",
    "GameCharAgentProject", "GameCharInterfaceProject", "TradepostAgent", "TradepostInterface",
    "TradepostAgentProject", "TradepostInterfaceProject",
]


def endpoint_hash(name):
    h = 0
    for c in name.encode():
        h = (h * 31 + c) & 0xFFFFFFFF
    return h


ENDPOINT_BY_HASH = {endpoint_hash(n): n for n in ENDPOINT_NAMES}


def is_funcom(ip):
    return ip.startswith(FUNCOM_PREFIXES)


# ---------------------------------------------------------------- TCP reassembly

class Direction:
    """One TCP direction of one connection."""

    def __init__(self, key):
        self.key = key  # (src, sport, dst, dport)
        self.segments = []  # (seq, payload, frame_number, ts_raw)
        self.syn_seq = None
        self.retransmits = 0
        self.gaps = 0
        self.runs = []  # list of Run

    def add(self, seq, payload, frame_no, ts, syn):
        if syn:
            self.syn_seq = seq
        if payload:
            self.segments.append((seq, payload, frame_no, ts))

    def assemble(self):
        """Build self.runs: contiguous byte runs separated by holes in the capture."""
        self.runs = []
        if not self.segments:
            return
        if self.syn_seq is not None:
            base = self.syn_seq + 1
        else:
            base = min(s for s, *_ in self.segments)  # connection started before capture
        self.segments.sort(key=lambda s: (s[0] - base) & 0xFFFFFFFF)
        cur = None  # current Run
        end = 0  # absolute stream offset just past the last byte taken
        for seq, payload, frame_no, ts in self.segments:
            rel = (seq - base) & 0xFFFFFFFF
            if rel > 0x7FFFFFFF:  # before the start of the stream
                continue
            if rel < end:
                overlap = end - rel
                if overlap >= len(payload):
                    self.retransmits += 1
                    continue
                payload = payload[overlap:]
            elif rel > end or cur is None:
                if cur is not None:
                    self.gaps += 1
                    self.runs.append(cur)
                cur = Run(start=rel, first_frame=frame_no, first_ts=ts)
                end = rel
            cur.segs.append((len(cur.data), frame_no, ts))
            cur.data += payload
            end += len(payload)
        if cur is not None:
            self.runs.append(cur)


class Run:
    def __init__(self, start, first_frame, first_ts):
        self.start = start  # stream offset of the first byte
        self.data = b""
        self.first_frame = first_frame
        self.first_ts = first_ts
        self.segs = []  # (offset in data, capture frame number, raw timestamp) per TCP segment

    def frame_at(self, off):
        """Capture frame number and raw timestamp of the segment holding byte `off`."""
        lo, hi = 0, len(self.segs) - 1
        best = self.segs[0] if self.segs else (0, self.first_frame, self.first_ts)
        while lo <= hi:
            mid = (lo + hi) // 2
            if self.segs[mid][0] <= off:
                best = self.segs[mid]
                lo = mid + 1
            else:
                hi = mid - 1
        return best[1], best[2]


def iter_capture(path):
    """Frames from a Network Monitor .cap, or a pcap/pcapng file (pcap.py)."""
    with open(path, "rb") as fh:
        magic = fh.read(4)
    return netmon.iter_frames(path) if magic == b"GMBU" else pcap.iter_frames(path)


def build_directions(path):
    """Return {(src,sport,dst,dport): Direction} for Funcom TCP flows, plus frame stats."""
    dirs = {}
    stats = collections.Counter()
    for fr in iter_capture(path):
        stats["frames"] += 1
        ip = fr.ip
        if ip is None or ip.tcp is None:
            continue
        if not (is_funcom(ip.src) or is_funcom(ip.dst)):
            continue
        t = ip.tcp
        key = (ip.src, t.sport, ip.dst, t.dport)
        d = dirs.setdefault(key, Direction(key))
        d.add(t.seq, t.payload, fr.number, fr.ts_raw, t.syn)
        stats["funcom_tcp_frames"] += 1
    for d in dirs.values():
        d.assemble()
    return dirs, stats


# ---------------------------------------------------------------- LMO frames

def _frame_at(data, pos):
    """Return (control, body, end) if a CRC-valid frame starts at pos, else None."""
    if pos + 8 > len(data):
        return None
    raw = struct.unpack_from(">I", data, pos)[0]
    length = raw & 0x7FFFFFFF
    if length < 4 or length > 0x100000 or pos + 4 + length > len(data):
        return None
    crc = struct.unpack_from(">I", data, pos + 4)[0]
    body = data[pos + 8:pos + 4 + length]
    if (binascii.crc32(body) & 0xFFFFFFFF) != crc:
        return None
    return bool(raw & 0x80000000), body, pos + 4 + length


def iter_frames_plain(data, resync=True):
    """Yield (offset, control, body) for CRC-valid frames. With resync, skip bytes that do
    not start a valid frame (holes, mid-stream start)."""
    pos = 0
    n = len(data)
    while pos + 8 <= n:
        got = _frame_at(data, pos)
        if got is None:
            if not resync:
                return
            pos += 1
            continue
        control, body, end = got
        yield pos, control, body
        pos = end


def decompress_zlib_runs(runs):
    """Inflate the server->client stream, which is one zlib stream across runs.
    Returns (bytes, lost, segmap): lost counts runs that could not be inflated; segmap is a
    sorted list of (inflated offset, capture frame, raw timestamp), one per TCP segment, so a
    message's arrival time is that of the segment that inflated its first byte."""
    out = bytearray()
    segmap = []
    lost = 0
    z = None
    for i, run in enumerate(runs):
        skip = 0
        if z is None:
            # Only the first run can start the stream: a control frame (9 bytes), then zlib.
            if i != 0 or len(run.data) <= 11 or run.data[9] != 0x78:
                lost += 1
                continue
            z = zlib.decompressobj()
            skip = 9
        bounds = [o for o, _, _ in run.segs] + [len(run.data)]
        try:
            for k, (off, frame_no, ts) in enumerate(run.segs):
                chunk = run.data[max(off, skip):bounds[k + 1]]
                if not chunk:
                    continue
                segmap.append((len(out), frame_no, ts))
                out += z.decompress(chunk)
        except zlib.error:
            lost += 1
            z = None  # cannot resume a zlib stream after a hole
    return bytes(out), lost, segmap


def segmap_at(segmap, off):
    """(frame, ts) of the segment that inflated byte `off` (see decompress_zlib_runs)."""
    lo, hi, best = 0, len(segmap) - 1, (None, None)
    while lo <= hi:
        mid = (lo + hi) // 2
        if segmap[mid][0] <= off:
            best = segmap[mid][1:]
            lo = mid + 1
        else:
            hi = mid - 1
    return best


# ---------------------------------------------------------------- protobuf (RpcHeader)

def _varint(b, p):
    shift = 0
    v = 0
    while True:
        c = b[p]
        p += 1
        v |= (c & 0x7F) << shift
        if not c & 0x80:
            return v, p
        shift += 7


def _pb_fields(b):
    """Yield (field_no, wire_type, value) for a protobuf message body."""
    p = 0
    while p < len(b):
        tag, p = _varint(b, p)
        fno, wt = tag >> 3, tag & 7
        if wt == 0:
            v, p = _varint(b, p)
        elif wt == 1:
            v = b[p:p + 8]
            p += 8
        elif wt == 2:
            ln, p = _varint(b, p)
            v = b[p:p + ln]
            p += ln
        elif wt == 5:
            v = b[p:p + 4]
            p += 4
        else:
            raise ValueError("unsupported protobuf wire type %d" % wt)
        yield fno, wt, v


def _endpoint(b):
    e = {"type": None, "instance": None, "token": None}
    for fno, wt, v in _pb_fields(b):
        if fno == 1 and wt == 5:
            e["type"] = struct.unpack("<I", v)[0]
        elif fno == 2:
            e["instance"] = v
        elif fno == 3:
            e["token"] = v
    return e


def parse_rpc_header(body):
    """Return (header dict, args bytes) for an RPC body (u32 hlen, RpcHeader, args)."""
    hlen = struct.unpack_from(">I", body, 0)[0]
    hdr_bytes = body[4:4 + hlen]
    args = body[4 + hlen:]
    hdr = {"caller": None, "endpoint": None, "routing": [], "method": None}
    for fno, wt, v in _pb_fields(hdr_bytes):
        if fno == 1:
            hdr["caller"] = _endpoint(v)
        elif fno == 2:
            hdr["endpoint"] = _endpoint(v)
        elif fno == 5:
            hdr["routing"].append(_endpoint(v))
        elif fno == 4:
            hdr["method"] = v
    return hdr, args


def endpoint_name(ep):
    if not ep or ep["type"] is None:
        return None
    return ENDPOINT_BY_HASH.get(ep["type"], "?0x%08x" % ep["type"])


# ---------------------------------------------------------------- method names

def load_method_names():
    with open(RPC_METHODS) as fh:
        return json.load(fh)


def method_signature(tables, direction, endpoint, method):
    """direction: 'S->C' (receiver tables, endpoint = client-side interface) or
    'C->S' (sender tables, endpoint = server-side agent/RPC). Method is the u32 method id."""
    if endpoint is None or method is None:
        return None
    if direction == "S->C":
        tbl = tables["receivers"].get(endpoint + "Receiver")
    else:
        tbl = tables["senders"].get(endpoint + "Sender")
    if not tbl:
        return None
    return tbl.get("0x%08x" % method) or tbl.get(str(method))


# ---------------------------------------------------------------- driver

def flow_messages(d):
    """Yield dicts for each decoded RPC in one TCP direction.
    Returns (messages, info) where info describes how the stream was read."""
    key = d.key
    src = key[0]
    direction = "S->C" if is_funcom(src) else "C->S"
    info = {"runs": len(d.runs), "gaps": d.gaps, "retransmits": d.retransmits,
            "bytes": sum(len(r.data) for r in d.runs)}
    frames = []
    if direction == "S->C" and d.runs and len(d.runs[0].data) > 11 and d.runs[0].data[9] == 0x78:
        info["mode"] = "zlib"
        data, lost, segmap = decompress_zlib_runs(d.runs)
        info["zlib_runs_lost"] = lost
        frames = [(o, c, b, 0, *segmap_at(segmap, o)) for o, c, b in iter_frames_plain(data, resync=False)]
    else:
        info["mode"] = "plain"
        for run_idx, run in enumerate(d.runs):
            for off, ctl, body in iter_frames_plain(run.data, resync=True):
                frame_no, ts = run.frame_at(off)
                frames.append((off, ctl, body, run_idx, frame_no, ts))
    return direction, frames, info


def census(path, jsonl=None):
    tables = load_method_names()
    dirs, stats = build_directions(path)
    counter = collections.Counter()
    out_f = open(jsonl, "w") if jsonl else None
    per_flow = {}
    for key, d in sorted(dirs.items(), key=lambda kv: kv[0]):
        direction, frames, info = flow_messages(d)
        server_ep = (key[2], key[3]) if direction == "C->S" else (key[0], key[1])
        fk = "%s %s:%d" % (direction, server_ep[0], server_ep[1])
        fc = dict(info)
        ok_frames = 0
        for off, ctl, body, run_idx, frame_no, ts in frames:
            if ctl:
                fc["control"] = fc.get("control", 0) + 1
                continue
            try:
                hdr, args = parse_rpc_header(body)
            except Exception:
                fc["header_error"] = fc.get("header_error", 0) + 1
                continue
            ok_frames += 1
            ep = endpoint_name(hdr["endpoint"])
            caller = endpoint_name(hdr["caller"])
            meth = hdr["method"]
            meth_i = int.from_bytes(meth, "big") if isinstance(meth, (bytes, bytearray)) else meth
            sig = method_signature(tables, direction, ep, meth_i)
            name = sig.split("(")[0] if sig else "0x%08x" % (meth_i or 0)
            counter["%s %s %s" % (direction, ep, name)] += 1
            if out_f:
                out_f.write(json.dumps({
                    "flow": fk, "dir": direction, "run": run_idx,
                    "frame": frame_no, "ts_raw": ts, "caller": caller, "endpoint": ep,
                    "method": sig or ("0x%08x" % (meth_i or 0)),
                    "args_hex": args.hex(), "args_len": len(args),
                }) + "\n")
        fc["rpc"] = ok_frames
        per_flow[fk] = fc
    if out_f:
        out_f.close()
    return {"stats": dict(stats), "flows": per_flow,
            "rpc_census": dict(counter.most_common())}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("--census", help="write census JSON here")
    ap.add_argument("--jsonl", help="write one decoded RPC per line here")
    a = ap.parse_args()
    result = census(a.capture, a.jsonl)
    text = json.dumps(result, indent=2)
    if a.census:
        with open(a.census, "w") as fh:
            fh.write(text + "\n")
    else:
        print(text)


if __name__ == "__main__":
    main()
