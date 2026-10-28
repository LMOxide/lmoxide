#!/usr/bin/env python3
"""Minimal reader for Microsoft Network Monitor 2.x capture files (.cap, "GMBU").

Layout (verified against the LMO live capture):
  header   : "GMBU", version bytes, MacType u16 (1 = Ethernet), SYSTEMTIME start,
             FrameTableOffset u32 (at byte 24), FrameTableLength u32 (bytes, 4 per frame)
  frame tbl: u32 LE offset per frame, in capture order
  record   : u64 LE timestamp, u32 orig length, u32 captured length, then captured bytes

Only Ethernet II, IPv4, TCP and UDP are decoded here; everything else is returned raw.
"""
import struct
import sys
from dataclasses import dataclass, field


@dataclass
class Frame:
    number: int
    ts_raw: int          # raw 64-bit timestamp from the record header (microseconds)
    orig_len: int
    data: bytes          # captured Ethernet frame
    ip: "IPv4 | None" = None


@dataclass
class IPv4:
    src: str
    dst: str
    proto: int
    ttl: int
    ident: int
    flags_frag: int
    payload: bytes
    tcp: "TCP | None" = None
    udp: "UDP | None" = None


@dataclass
class TCP:
    sport: int
    dport: int
    seq: int
    ack: int
    flags: int
    payload: bytes

    @property
    def syn(self):
        return bool(self.flags & 0x02)

    @property
    def fin(self):
        return bool(self.flags & 0x01)

    @property
    def rst(self):
        return bool(self.flags & 0x04)


@dataclass
class UDP:
    sport: int
    dport: int
    payload: bytes


def _ip(b):
    return ".".join(str(x) for x in b)


def decode_frame(number, ts_raw, orig_len, data):
    f = Frame(number, ts_raw, orig_len, data)
    if len(data) < 14:
        return f
    ethertype = struct.unpack_from(">H", data, 12)[0]
    if ethertype == 0x0800 and len(data) >= 34:
        ihl = (data[14] & 0x0F) * 4
        total = struct.unpack_from(">H", data, 16)[0]
        ident, ff = struct.unpack_from(">HH", data, 18)
        proto = data[23]
        ip = IPv4(
            src=_ip(data[26:30]),
            dst=_ip(data[30:34]),
            proto=proto,
            ttl=data[22],
            ident=ident,
            flags_frag=ff,
            payload=data[14 + ihl:14 + total] if total else data[14 + ihl:],
        )
        p = ip.payload
        if proto == 6 and len(p) >= 20:
            sport, dport, seq, ack = struct.unpack_from(">HHII", p, 0)
            doff = (p[12] >> 4) * 4
            ip.tcp = TCP(sport, dport, seq, ack, p[13], p[doff:])
        elif proto == 17 and len(p) >= 8:
            sport, dport, ulen = struct.unpack_from(">HHH", p, 0)
            ip.udp = UDP(sport, dport, p[8:ulen] if ulen >= 8 else p[8:])
        f.ip = ip
    return f


def iter_frames(path):
    """Yield decoded Frame objects in frame-table (capture) order."""
    with open(path, "rb") as fh:
        d = fh.read()
    if d[:4] != b"GMBU":
        raise ValueError(f"{path}: not a Network Monitor capture (magic {d[:4]!r})")
    fto, ftl = struct.unpack_from("<II", d, 24)
    count = ftl // 4
    offsets = struct.unpack_from("<%dI" % count, d, fto)
    for n, o in enumerate(offsets, 1):
        ts_raw, orig, incl = struct.unpack_from("<QII", d, o)
        data = d[o + 16:o + 16 + incl]
        yield decode_frame(n, ts_raw, orig, data)


if __name__ == "__main__":
    path = sys.argv[1]
    total = 0
    protos = {}
    for fr in iter_frames(path):
        total += 1
        key = "ip" if fr.ip else "non-ip"
        protos[key] = protos.get(key, 0) + 1
    print(total, protos)
