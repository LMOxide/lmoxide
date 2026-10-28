#!/usr/bin/env python3
"""Minimal reader for libpcap (.pcap) and pcapng capture files.

Yields the same netmon.Frame objects as netmon.iter_frames, so livecap.py can decode either
format. ts_raw is in microseconds since the Unix epoch (pcapng: converted from the interface's
if_tsresol). Only Ethernet (linktype 1) and raw IPv4 (101/228) links are decoded.
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import netmon  # noqa: E402

ETHERNET, RAW_IP, IPV4 = 1, 101, 228


def _frame(number, ts_us, orig_len, data, linktype):
    if linktype in (RAW_IP, IPV4):
        data = b"\0" * 12 + b"\x08\x00" + data  # fake Ethernet header
    return netmon.decode_frame(number, ts_us, orig_len, data)


def _iter_pcap(d):
    magic = d[:4]
    e = "<" if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1") else ">"
    nano = magic in (b"\x4d\x3c\xb2\xa1", b"\xa1\xb2\x3c\x4d")
    linktype = struct.unpack_from(e + "I", d, 20)[0]
    p, n = 24, 0
    while p + 16 <= len(d):
        sec, frac, incl, orig = struct.unpack_from(e + "IIII", d, p)
        n += 1
        ts = sec * 1_000_000 + (frac // 1000 if nano else frac)
        yield _frame(n, ts, orig, d[p + 16:p + 16 + incl], linktype)
        p += 16 + incl


def _iter_pcapng(d):
    e = "<"
    links = []  # (linktype, ticks per second) per interface
    p, n = 0, 0
    while p + 12 <= len(d):
        btype = struct.unpack_from(e + "I", d, p)[0]
        if btype == 0x0A0D0D0A:
            e = "<" if d[p + 8:p + 12] == b"\x4d\x3c\x2b\x1a" else ">"
            links = []
        blen = struct.unpack_from(e + "I", d, p + 4)[0]
        body = d[p + 8:p + blen - 4]
        if btype == 1:  # Interface Description
            linktype = struct.unpack_from(e + "H", body, 0)[0]
            res = 1_000_000
            q = 8
            while q + 4 <= len(body):
                code, ln = struct.unpack_from(e + "HH", body, q)
                if code == 0:
                    break
                if code == 9:  # if_tsresol
                    r = body[q + 4]
                    res = 2 ** (r & 0x7F) if r & 0x80 else 10 ** r
                q += 4 + ((ln + 3) & ~3)
            links.append((linktype, res))
        elif btype == 6:  # Enhanced Packet
            iface, hi, lo, incl, orig = struct.unpack_from(e + "IIIII", body, 0)
            linktype, res = links[iface]
            n += 1
            ts = ((hi << 32) | lo) * 1_000_000 // res
            yield _frame(n, ts, orig, body[20:20 + incl], linktype)
        elif btype == 3:  # Simple Packet
            orig = struct.unpack_from(e + "I", body, 0)[0]
            n += 1
            yield _frame(n, 0, orig, body[4:4 + orig], links[0][0])
        p += blen


def iter_frames(path):
    with open(path, "rb") as fh:
        d = fh.read()
    if d[:4] == b"\x0a\x0d\x0d\x0a":
        return _iter_pcapng(d)
    if d[:4] in (b"\xd4\xc3\xb2\xa1", b"\xa1\xb2\xc3\xd4", b"\x4d\x3c\xb2\xa1", b"\xa1\xb2\x3c\x4d"):
        return _iter_pcap(d)
    raise ValueError(f"{path}: not a pcap/pcapng file (magic {d[:4]!r})")


if __name__ == "__main__":
    for fr in iter_frames(sys.argv[1]):
        ip = fr.ip
        if ip and ip.tcp:
            t = ip.tcp
            print(fr.number, fr.ts_raw, ip.src, t.sport, ip.dst, t.dport, "%02x" % t.flags, len(t.payload))
        elif ip and ip.udp:
            print(fr.number, fr.ts_raw, ip.src, ip.udp.sport, ip.dst, ip.udp.dport, "udp", len(ip.udp.payload))
        else:
            print(fr.number, fr.ts_raw, "non-ip", len(fr.data))
