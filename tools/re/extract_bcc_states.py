"""BCCState names: the values of character stat 20 (BCCState, Control_n::BCTStates_e) and the
BCC substates (Control_n::BCTSubstates_e, BehaviorControlCenterIIR_c actions 11/12), from the
client's character behaviour tree. The tree is RDB type 1010203 (XML text, the only instance is
12488, "CharacterBcc") / 1010501 id 1 (the same as LMXB binary XML, e_RDB_Res_BCCTree_Binary),
both loaded by BCCTree_c::Load (Linux LMO 0x19c79c8). Each <state id name> / <substate id name
priority> lists the action groups it inherits (<inherit group grouprdbid>, BCTGroup resources).
Writes data/bcc_states.json.  Usage: tools/re/py extract_bcc_states.py"""
import json, os
import xml.etree.ElementTree as ET

from rdb import body, index

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
out_path = os.path.join(ROOT, "data/bcc_states.json")
TREE_TYPE, TREE_ID = 1010203, 12488

# what is known of the client's behaviour in a state (server code, live captures)
ABOUT = {
    1: "normal: what a player is in when doing nothing special (server BCC_STATE_NORMAL)",
    4: "in combat (BehaviorControlCenter::IsFightState is true for it)",
    29: "smashed/dead: death effect, the 'You got smashed!' window for a player; only the Limbo "
        "action group",
    104: "building a buildable: the client asks for it with BCC command 1 (data 104) and the "
         "server sets it, back to 1 when the build stops",
}


def entries(root, tag):
    out = []
    for e in root.iter(tag):
        v = int(e.get("id"))
        groups = [i.get("group") for i in e.iter("inherit")]
        d = {"value": v, "name": e.get("name")}
        if e.get("priority"):
            d["priority"] = int(e.get("priority"))
        d["groups"] = groups
        if tag == "state":
            d["about"] = ABOUT.get(v, "")
        out.append(d)
    return sorted(out, key=lambda d: d["value"])


def main():
    root = ET.fromstring(body(TREE_TYPE, TREE_ID, index()).decode("latin-1"))
    data = {
        "_comment": "Names from the client's character behaviour tree (RDB 1010203/12488 "
                    "\"CharacterBcc\" XML, same as 1010501/1 binary, loaded by BCCTree_c::Load); "
                    "the only states and substates the client defines for player characters, "
                    "extracted by tools/re/extract_bcc_states.py. groups = inherited action "
                    "groups (what the character may do in it); about = observed behaviour.",
        "states": entries(root, "state"),
        "substates": entries(root, "substate"),
    }
    with open(out_path, "w") as f:
        f.write("{\n" + f' "_comment": {json.dumps(data["_comment"])},\n')
        for k in ("states", "substates"):
            rows = ",\n".join(f"  {json.dumps(r)}" for r in data[k])
            f.write(f' "{k}": [\n{rows}\n ]' + (",\n" if k == "states" else "\n"))
        f.write("}\n")
    print(f"{len(data['states'])} states, {len(data['substates'])} substates -> {out_path}")


if __name__ == "__main__":
    main()
