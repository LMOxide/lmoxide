#!/usr/bin/env python3
"""Patch a client executable (LMO.exe or the Linux LMO) for an LMOxide server (reversible).

- Chat filter: before sending a chat line the client asks
  http://chatfilter.funcom.com/rest/chat_filter?... (ProjectChatServerInterface::SendMessage)
  and only sends it if that answers result 0. The URL is a format string
  ("http://chatfilter.funcom.com/rest/%s", 36 chars); it becomes "http://<host>/rest/%s".
- Login key (--login-key): the client encrypts the password against a server public key
  compiled in (global `session`, 512 hex digits; Funcom's, whose private half is lost). It is
  replaced by the LMOxide server's key (`lmoxide-server login-key`), or fetched from the server
  with --login-key-url (the patch server's /lmoxide/v1/login-key).
- Private messages (Linux build): 1.5 emptied ChatServerInterface_c::DoTell, so "/tell" did
  nothing; it becomes a call of the vtable's working SendTell.
- Chat for everyone: Data/Gui/Default/Modules.xml (next to the exe, stock copy kept as .orig)
  loses the ChatGUI module's GM-only condition `stat:gmlevel!=0` (added in 1.5).

lmoxide-patch-server does the same to the binaries it serves; this script is for clients
installed by hand. Every run starts from the stock copy, kept as <exe>.orig.

  patches/patch-client.py [--exe PATH] [--host 127.0.0.1:8080] [--login-key HEX | --login-key-url URL]
  patches/patch-client.py --revert
"""
import argparse, os, re, shutil, sys, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_EXE = os.path.join(HERE, "../../clients/windows/LEGO Minifigures Online/LMO.exe")
CHAT_FILTER = b"http://chatfilter.funcom.com/rest/%s"
# empty DoTell, then SetCookie; DoTell -> mov rax,[rdi]; jmp [rax+0x18] (SendTell)
DO_TELL = bytes.fromhex("554889e55dc3" "554889e58977185dc3")
DO_TELL_PATCHED = bytes.fromhex("488b07ff6018" "554889e58977185dc3")
MODULES = os.path.join("Data", "Gui", "Default", "Modules.xml")
CHAT_GUI_GM = "stat:gmlevel!=0 &amp;&amp; !fullscreen_menu"
# lmoxide_net::login::FUNCOM_SESSION_KEY
FUNCOM_KEY = (
    b"a738d2bc4ba9fea7a6d3e485f85f5a5c74c459940c488f8ec88ab5a326161c133bbaee9c1ee7fd0e0453b51cc8139826"
    b"e0e7c1bdecc5682e2af019d862026133f4e1239d331365b3643991add017cd1039104f44e0628e0045264900a594d18d"
    b"7d4914493c4c2e03e667ef9268bceae22305d0ec7dc7422346df3067569cda599013ddab68c64a16ecb796e1eb11276b"
    b"3fb90800c3e5b56f34e570f8683e3b3f333acebc9d988d667726a61bbd2487d8e97bf371a783a5284eb3d2c0a768cc1f"
    b"138992ebda48a81d89762216a589b7963d1df15396a3b8b7b9cbb6aa6354d2aa3a4bce05d6a5207dc7cead7567ae4d05"
    b"498d97a076f4d2ffb9469ac23785cf2b"
)


def replace_once(data, old, new, what):
    at = data.find(old)
    if at < 0 or data.find(old, at + 1) >= 0:
        sys.exit(f"{what}: expected exactly one occurrence in the stock executable")
    data[at : at + len(old)] = new
    return at


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=DEFAULT_EXE)
    ap.add_argument("--host", default="127.0.0.1:8080", help="the patch server, host:port (chat filter)")
    ap.add_argument("--login-key", help="the server's login public key (hex)")
    ap.add_argument("--login-key-url", help="fetch the login key, e.g. http://host:8080/lmoxide/v1/login-key")
    ap.add_argument("--revert", action="store_true")
    a = ap.parse_args()
    backup = a.exe + ".orig"
    if a.revert:
        if not os.path.exists(backup):
            sys.exit("no backup to restore")
        shutil.copy2(backup, a.exe)
        print("restored", a.exe)
        modules = os.path.join(os.path.dirname(os.path.abspath(a.exe)), MODULES)
        if os.path.exists(modules + ".orig"):
            shutil.copy2(modules + ".orig", modules)
            print("restored", modules)
        return
    if not os.path.exists(backup):
        shutil.copy2(a.exe, backup)
    data = bytearray(open(backup, "rb").read())

    url = f"http://{a.host}/rest/%s".encode()
    if len(url) > len(CHAT_FILTER):
        sys.exit(f"URL too long ({len(url)} > {len(CHAT_FILTER)} bytes)")
    at = replace_once(data, CHAT_FILTER + b"\0", url.ljust(len(CHAT_FILTER) + 1, b"\0"), "chat filter URL")
    print(f"patched chat filter URL at file offset {at:#x} -> {url.decode()}")

    key = a.login_key
    if a.login_key_url:
        with urllib.request.urlopen(a.login_key_url, timeout=10) as r:
            key = r.read().decode().strip()
    if key:
        key = key.strip().lower().zfill(len(FUNCOM_KEY))
        if not re.fullmatch(r"[0-9a-f]+", key) or len(key) != len(FUNCOM_KEY):
            sys.exit("the login key must be at most 512 hex digits")
        at = replace_once(data, FUNCOM_KEY, key.encode(), "login key")
        print(f"patched login key at file offset {at:#x}")
    else:
        print("no --login-key: the client keeps Funcom's key and can't log in to an LMOxide server", file=sys.stderr)

    if data.count(DO_TELL) == 1:
        at = replace_once(data, DO_TELL, DO_TELL_PATCHED, "DoTell")
        print(f"patched DoTell (private messages) at file offset {at:#x}")
    else:
        print("DoTell not found (only the Linux build is patched): no private messages", file=sys.stderr)

    patch_modules(os.path.join(os.path.dirname(os.path.abspath(a.exe)), MODULES))

    tmp = a.exe + ".tmp"
    open(tmp, "wb").write(data)
    shutil.copymode(backup, tmp)
    os.replace(tmp, a.exe)


def patch_modules(path):
    """The chat window for everyone: drop the ChatGUI module's GM condition."""
    if not os.path.exists(path):
        print(f"no {path}: chat stays GM-only", file=sys.stderr)
        return
    if not os.path.exists(path + ".orig"):
        shutil.copy2(path, path + ".orig")
    xml = open(path + ".orig", encoding="utf-8", newline="").read()
    if xml.count(CHAT_GUI_GM) != 1:
        print(f"{path}: GM condition not found; left as is", file=sys.stderr)
        return
    open(path, "w", encoding="utf-8", newline="").write(xml.replace(CHAT_GUI_GM, "!fullscreen_menu"))
    print(f"patched {path}: chat for everyone")


if __name__ == "__main__":
    main()
