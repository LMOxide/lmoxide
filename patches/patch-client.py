#!/usr/bin/env python3
"""Point the client's hard-coded chat filter URL at our HTTP server (reversible).

Before sending a chat line the client asks http://chatfilter.funcom.com/rest/chat_filter?...
(ProjectChatServerInterface::SendMessage) and only sends it if that answers result 0. The URL
is a format string in LMO.exe ("http://chatfilter.funcom.com/rest/%s", 36 chars); this
overwrites it in place with "http://<host>/rest/%s". A backup is kept as LMO.exe.orig.

  patches/patch-client.py [--host 127.0.0.1:8080] [--exe PATH]
  patches/patch-client.py --revert
"""
import argparse, os, shutil, sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_EXE = os.path.join(HERE, "../../clients/windows/LEGO Minifigures Online/LMO.exe")
ORIGINAL = b"http://chatfilter.funcom.com/rest/%s"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=DEFAULT_EXE)
    ap.add_argument("--host", default="127.0.0.1:8080", help="our HTTP server, host:port")
    ap.add_argument("--revert", action="store_true")
    a = ap.parse_args()
    backup = a.exe + ".orig"
    if a.revert:
        if not os.path.exists(backup):
            sys.exit("no backup to restore")
        shutil.copy2(backup, a.exe)
        print("restored", a.exe)
        return
    data = bytearray(open(a.exe, "rb").read())
    url = f"http://{a.host}/rest/%s".encode()
    if len(url) > len(ORIGINAL):
        sys.exit(f"URL too long ({len(url)} > {len(ORIGINAL)} bytes)")
    at = data.find(ORIGINAL)
    if at < 0:
        # already patched? find our previous patch by its suffix
        at = data.find(b"/rest/%s\0")
        while at > 0 and data[at - 1] != 0:
            at -= 1
        if at <= 0 or not data[at:].startswith(b"http://"):
            sys.exit("chat filter URL not found")
    if not os.path.exists(backup):
        shutil.copy2(a.exe, backup)
    data[at : at + len(ORIGINAL) + 1] = url.ljust(len(ORIGINAL) + 1, b"\0")
    open(a.exe, "wb").write(data)
    print(f"patched chat filter URL at file offset {at:#x} -> {url.decode()}")


if __name__ == "__main__":
    main()
