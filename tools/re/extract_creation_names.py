"""The creation screen's name wheels (RDB 1010700/1, CharacterCreationData_c::LoadResource
0x1741b0c) into data/creation_names.json: first names (wheel 1), nicknames (MiddleNames, wheel 2)
and surnames (LastNames, wheel 3), in the client's order. The server picks the next free surname
when a created name is taken (game.rs SetCharacterCreationData).

  python3 tools/re/extract_creation_names.py
"""
import json, os, re

import rdb

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "../../data/creation_names.json")


def main():
    xml = rdb.body(1010700, 1).decode("utf-8")
    lists = {}
    for key, tag in (("first", "FirstNames"), ("nick", "MiddleNames"), ("last", "LastNames")):
        body = re.search(rf"<{tag}>(.*?)</{tag}>", xml, re.S).group(1)
        lists[key] = re.findall(r'value="([^"]*)"', body)
    with open(OUT, "w") as f:
        json.dump(lists, f, indent=0)
        f.write("\n")
    print({k: len(v) for k, v in lists.items()}, "->", os.path.relpath(OUT))


if __name__ == "__main__":
    main()
