#!/usr/bin/env python3
"""Conversation portraits: `render.icon` of the content's NPC templates (npc_stats.icon; the
conversation window shows the speaker's Icon stat, the default 25278 "DJ" without one,
Conversation.swf ConversationWindow.SlotActiveCinematicActorSet). Rerunnable; run
`cargo run -p lmoxide-content -- fmt` afterwards.

  - an npc_template the live capture shows: its icon there, or none (live's Tavern Pirates,
    Commoners, Mermaid, Captain Dreadleg had none);
  - otherwise the Icon stat (79) of the client's NPC template (RDB 1000620, dynel type 30035)
    with the NPC's name, for NPCs (kind npc) only.

  python3 tools/content/npc_icons.py [content dir]
"""
import collections
import glob
import json
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "re"))
import rdb  # noqa: E402

CONTENT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "content")
CAPTURE = os.path.join(HERE, "..", "..", "data", "live-capture", "lmo-captures", "playfield-objects.json")
NPC_TEMPLATE = 30035
ICON = 79


def client_icons():
    """Display name -> Icon of the client's NPC templates (u32 version, u32 dynel type, ...,
    u32 n at 20, n x (u32 stat, u8 kind, value: u32, or u32 length + text when kind is 2),
    then u16 length + display name)."""
    idx = rdb.index()
    icons = collections.defaultdict(set)
    for t, r in idx:
        if t != 1000620:
            continue
        try:
            b = rdb.body(t, r, idx)
        except Exception:
            continue
        if len(b) < 24 or struct.unpack_from("<I", b, 4)[0] != NPC_TEMPLATE:
            continue
        n = struct.unpack_from("<I", b, 20)[0]
        p, stats = 24, {}
        for _ in range(n):
            s, kind, v = struct.unpack_from("<IBI", b, p)
            p += 9
            if kind == 2:
                p += v
            else:
                stats[s] = v
        ln = struct.unpack_from("<H", b, p)[0]
        name = b[p + 2:p + 2 + ln].decode("latin1")
        if stats.get(ICON):
            icons[name].add(stats[ICON])
    return icons


def live_icons():
    """npc_template -> icon (None: live sent none) from the live capture."""
    out = {}
    with open(CAPTURE) as f:
        for o in json.load(f)["objects"]:
            npc = (o.get("core") or {}).get("npc_stats")
            if npc and npc.get("template_id") is not None:
                out[npc["template_id"]] = npc.get("icon")
    return out


def main():
    client, live = client_icons(), live_icons()
    changed = 0
    for path in sorted(glob.glob(os.path.join(CONTENT, "npcs", "*.json"))):
        with open(path) as f:
            d = json.load(f)
        render = d.get("render") or {}
        tpl = render.get("npc_template")
        if tpl in live:
            icon = live[tpl]
        elif d.get("kind") == "npc" and len(client.get(d.get("name"), ())) == 1:
            icon = next(iter(client[d["name"]]))
        else:
            icon = None
        if render.get("icon") == icon:
            continue
        if icon is None:
            render.pop("icon", None)
        else:
            render["icon"] = icon
        d["render"] = render
        with open(path, "w") as f:
            json.dump(d, f, indent=2)
            f.write("\n")
        changed += 1
        print(f"{os.path.basename(path)}: {d.get('name')} icon {icon}")
    print(f"{changed} templates changed")


if __name__ == "__main__":
    main()
