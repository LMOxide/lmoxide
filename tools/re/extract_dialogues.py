"""NPC dialogues (Knubot) for crates/lmoxide-server/src/knubot.rs. Writes data/dialogues.json:

  texts:     {id: text} of TDB category 50002 (the dialogue texts; their {tags} are the server
             script the original server ran, see knubot.rs)
  answers:   {text id: 50001 answer id} the answer whose wording is the text's spoken line (the
             button that leads to that text)
  keys:      {"category/id" or "category/token": key} the `key` attributes of localized
             references in the recorded session (the client's remoteformat XML carries them)
  names:     {51000 id: NPC name} of the dialogue owners
  dialogues: the recorded dialogues in order: {npc_name (51000 id), npc, playfield, texts}

The dialogue graph (which NPC, when) was server-side; only the recorded session's dialogues
(the tutorial and the start of Pirate World) are known. Their texts chain: each answer list
offered the next text's line and "Goodbye".

Usage: tools/re/py extract_dialogues.py <dir with recs.pkl from tools/re/demo/recs.py>"""
import html, json, os, pickle, re, struct, sys

import tdc

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
recs = pickle.load(open(os.path.join(sys.argv[1], "recs.pkl"), "rb"))

texts = tdc.texts(50002)
answers_db = tdc.texts(50001)
names_db = tdc.texts(51000)


def spoken(text):
    """The text's last spoken line, without tags and [speaker] marks."""
    lines = [re.sub(r"\{[^}]*\}|\[[^\]]*\]", "", l).strip() for l in text.splitlines()]
    lines = [l for l in lines if l]
    return lines[-1] if lines else ""


by_line = {}
for aid, a in answers_db.items():
    by_line.setdefault(a.strip(), aid)
answers = {tid: by_line[spoken(t)] for tid, t in texts.items() if spoken(t) in by_line}

# keys of localized references (the XML is escaped once inside parameter values)
keys = {}
for x in recs:
    if x["dir"] != "S>C" or not x["name"].startswith("Knubot"):
        continue
    xml = x["body"].decode("latin-1")
    for level in (xml, html.unescape(xml)):
        for m in re.finditer(r'<(?:remoteformat|localized) (?:id="(\d+)"|token="([^"]+)") category="(\d+)"[^>]*? key="([^"]*)"', level):
            ident = m.group(1) or m.group(2)
            keys.setdefault(f"{m.group(3)}/{ident}", html.unescape(m.group(4)))

# the recorded dialogues: Open ... Close per NPC
playfield = None
dialogues, current = [], None
for x in recs:
    if x["dir"] == "S>C" and x["name"] == "PlayfieldFIIR_t":
        # PlayfieldProxyData: model_id {type 51100, instance <playfield>}
        b = x["body"]
        j = b.find(b"\x08\x9c\x8f\x03\x10")
        if j >= 0:
            v = shift = 0
            for c in b[j + 5 :]:
                v |= (c & 0x7F) << shift
                shift += 7
                if not c & 0x80:
                    break
            playfield = v
    if x["dir"] != "S>C":
        continue
    if x["name"] == "KnubotOpenChatWindowIIR":
        current = {"texts": [], "playfield": playfield}
    elif x["name"] == "KnubotAppendTextIIR" and current is not None:
        xml = x["body"].decode("latin-1")
        tid = int(re.search(r'<remoteformat id="(\d+)" category="50002"', xml).group(1))
        if tid not in current["texts"]:
            current["texts"].append(tid)
        m = re.search(r'localized id=&quot;(\d+)&quot; category=&quot;51000&quot;', xml)
        if m:
            current["npc_name"] = int(m.group(1))
    elif x["name"] == "KnubotCloseChatWindowIIR" and current is not None:
        current["npc"] = names_db.get(current.get("npc_name"), "")
        dialogues.append(current)
        current = None

used = {d["npc_name"] for d in dialogues if "npc_name" in d}
out = {
    "_comment": __doc__.split("\n\n")[0].replace("\n", " "),
    "texts": {str(k): v for k, v in sorted(texts.items())},
    "answers": {str(k): v for k, v in sorted(answers.items())},
    "keys": dict(sorted(keys.items())),
    "names": {str(k): names_db[k] for k in sorted(used)},
    "dialogues": dialogues,
}
path = os.path.join(ROOT, "data/dialogues.json")
json.dump(out, open(path, "w"), indent=1, ensure_ascii=False)
print(f"{len(texts)} texts, {len(answers)} answers, {len(keys)} keys, {len(dialogues)} dialogues -> {path}")
for d in dialogues:
    print(f"  {d['playfield']} {d['npc']}: {d['texts']}")
