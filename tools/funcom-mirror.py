#!/usr/bin/env python3
"""Mirror Funcom's LMO patch CDN (http://update.lego.funcom.com/legoupm) to a local folder.

Files are saved byte for byte at their URL paths, so serving the folder over HTTP
(tools/funcom-cdn.sh) gives a drop-in replacement for the CDN: point the patch server's
--upstream (LMOXIDE_UPSTREAM) or a stock client's HttpPatchAddr at it.

What is fetched (layout in docs/PROTOCOL.md, "HTTP patch layout"):
  rdb/full/<md5>        every hash in PatchInfoClient.txt (RDBHashIndex.bin, all download lists)
  client/<h2>/<rest>    every platform's FileList.bin (RootHash-*), PatcherSetupHash-*, and
                        every file those lists name
  rdb/res/<h2>/<h1>/..  every resource in RDBHashIndex.bin and in every download list
                        (PC, Mac, Linux, Android and iOS texture variants)

Each body is checked against the md5 in its URL (after unwrapping IOz1 zlib / IOz2 LZMA).
Reruns skip files already present, so an interrupted run resumes. Failures are listed in
<out>/mirror-failures.txt; a summary goes to <out>/mirror-report.txt.

  tools/funcom-mirror.py [--out DIR] [--jobs N] [--only full,client,res]
"""
import argparse, hashlib, lzma, os, re, struct, sys, threading, time, urllib.error, urllib.request, zlib
from concurrent.futures import ThreadPoolExecutor, as_completed

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "../.."))
BASE = "http://update.lego.funcom.com/legoupm"
PATCH_INFO = os.path.join(HERE, "../data/PatchInfoClient.txt")


def unwrap(b):
    """Body as the client stores it: IOz1 = zlib, IOz2 = raw LZMA1 (props byte, u32 dict)."""
    if b[:4] == b"IOz1":
        return zlib.decompress(b[8:])
    if b[:4] == b"IOz2":
        size, p, dict_size = struct.unpack_from("<IBI", b, 4)
        f = {"id": lzma.FILTER_LZMA1, "lc": p % 9, "lp": p // 9 % 5, "pb": p // 45, "dict_size": dict_size}
        return lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=[f]).decompress(b[13:], size)
    return b


def res_path(h):
    return f"rdb/res/{h[:2]}/{h[2]}/{h[3:]}"


def client_path(h):
    return f"client/{h[:2]}/{h[2:]}"


class Mirror:
    def __init__(self, base, out, jobs):
        self.base, self.out, self.jobs = base.rstrip("/"), out, jobs
        self.lock = threading.Lock()
        self.failures, self.failed, self.fetched, self.bytes, self.skipped = [], set(), 0, 0, 0

    def local(self, rel):
        return os.path.join(self.out, rel)

    def get(self, rel, md5=None):
        """Raw body of `rel`, from the mirror or the CDN (saved there); None if unavailable."""
        path = self.local(rel)
        if os.path.exists(path):
            with self.lock:
                self.skipped += 1
            return open(path, "rb").read()
        if rel in self.failed:
            return None
        data, err = None, None
        for attempt in range(4):
            try:
                with urllib.request.urlopen(f"{self.base}/{rel}", timeout=120) as r:
                    data = r.read()
                break
            except urllib.error.HTTPError as e:
                err = f"HTTP {e.code}"
                if e.code in (403, 404):
                    break
            except Exception as e:  # timeouts, resets: retry
                err = str(e)
            time.sleep(2 * (attempt + 1))
        if data is None:
            return self.fail(rel, err)
        if md5:
            try:
                got = hashlib.md5(unwrap(data)).hexdigest()
            except Exception as e:
                return self.fail(rel, f"undecodable: {e}")
            if got != md5:
                return self.fail(rel, f"md5 {got}")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = f"{path}.part"
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
        with self.lock:
            self.fetched += 1
            self.bytes += len(data)
        return data

    def fail(self, rel, why):
        with self.lock:
            self.failures.append(f"{rel}\t{why}")
            self.failed.add(rel)
        return None

    def get_all(self, label, rels):
        """Fetches (rel, md5) pairs in parallel with a progress line."""
        todo = [(r, m) for r, m in rels if not os.path.exists(self.local(r))]
        print(f"{label}: {len(rels)} files, {len(todo)} to fetch", flush=True)
        done, start = 0, time.time()
        with ThreadPoolExecutor(self.jobs) as pool:
            for _ in as_completed([pool.submit(self.get, r, m) for r, m in todo]):
                done += 1
                if done % 500 == 0 or done == len(todo):
                    rate = self.bytes / max(time.time() - start, 1) / 1e6
                    print(f"  {done}/{len(todo)}  {self.bytes / 1e9:.2f} GB  {rate:.1f} MB/s  "
                          f"{len(self.failures)} failed", flush=True)


def patch_info(text):
    return dict(l.strip().split("=", 1) for l in text.splitlines() if "=" in l)


def rdb_index(data):
    """{md5} of every resource in RDBHashIndex.bin (format in tools/re/rdb.py)."""
    assert data[:4] == b"RDHI", data[:4]
    out, o = set(), 16
    for _ in range(struct.unpack_from("<I", data, 12)[0]):
        _, n = struct.unpack_from("<2I", data, o)
        o += 8
        for _ in range(n):
            out.add(data[o + 12 : o + 28].hex())
            o += 47
    return out


def download_list(data):
    return set(re.findall(rb"<Hash [^>]*>([0-9a-fA-F]{32})</Hash>", data))


def file_list(data):
    """{md5} of every file in a FileList.bin ("CFHL"; layout in lmoxide-patch-server stock.rs)."""
    if data[:8] != b"CFHL\x01\0\0\0":
        return set()
    out = set()

    def entries(at, n):
        for _ in range(n):
            kind, ln = data[at], struct.unpack_from("<H", data, at + 1)[0]
            at += 3 + ln
            if kind == 1:
                at = entries(at + 2, struct.unpack_from("<H", data, at)[0])
            else:
                out.add(data[at + 4 : at + 20].hex())
                at += 20
        return at

    entries(10, struct.unpack_from("<H", data, 8)[0])
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--out", default=os.path.join(ROOT, "clients/funcom-cdn/legoupm"))
    ap.add_argument("--patch-info", default=PATCH_INFO, help="PatchInfoClient.txt naming the hashes")
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--only", default="full,client,res")
    args = ap.parse_args()
    only = set(args.only.split(","))
    m = Mirror(args.base, args.out, args.jobs)
    os.makedirs(args.out, exist_ok=True)
    text = open(args.patch_info).read()
    open(os.path.join(args.out, "PatchInfoClient.txt"), "w").write(text)
    info = patch_info(text)

    full = sorted({v for k, v in info.items() if k == "RDBHash" or k.startswith("DownloadList-")})
    roots = sorted({v for k, v in info.items() if k.startswith(("RootHash", "PatcherSetupHash"))})
    m.get_all("rdb/full (indexes, download lists)", [(f"rdb/full/{h}", h) for h in full])

    res = set()
    idx = m.get(f"rdb/full/{info['RDBHash']}", info["RDBHash"])
    if idx:
        res |= rdb_index(unwrap(idx))
    print(f"RDBHashIndex.bin: {len(res)} resources")
    for k, h in sorted(info.items()):
        if k.startswith("DownloadList-") and (data := m.get(f"rdb/full/{h}", h)):
            hashes = {x.decode().lower() for x in download_list(unwrap(data))}
            print(f"{k}: {len(hashes)} resources, {len(hashes - res)} not in the index")
            res |= hashes

    client = set()
    if "client" in only:
        m.get_all("client file lists, patcher setups", [(client_path(h), h) for h in roots])
        for h in roots:
            if (data := m.get(client_path(h), h)) and (names := file_list(unwrap(data))):
                print(f"FileList.bin {h}: {len(names)} files")
                client |= names
        m.get_all("client files", [(client_path(h), h) for h in sorted(client)])
    if "res" in only:
        m.get_all("rdb/res (resources)", [(res_path(h), h) for h in sorted(res)])

    fails = sorted(m.failures)
    open(os.path.join(args.out, "mirror-failures.txt"), "w").write("".join(f"{f}\n" for f in fails))
    report = (f"{time.strftime('%Y-%m-%d %H:%M:%S')} from {m.base}\n"
              f"rdb/full {len(full)}, client {len(roots) + len(client)}, rdb/res {len(res)}\n"
              f"fetched {m.fetched} ({m.bytes / 1e9:.2f} GB), failed {len(fails)}\n")
    open(os.path.join(args.out, "mirror-report.txt"), "a").write(report + "\n")
    print(report, end="")
    if fails:
        print(f"failures: {os.path.join(args.out, 'mirror-failures.txt')}")


if __name__ == "__main__":
    sys.exit(main())
