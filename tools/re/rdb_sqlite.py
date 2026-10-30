"""Build a read-only SQLite database of everything the client stores: every RDB resource, the
RDB index, and the loose files in the client's Data/ folder.

  tools/re/py rdb_sqlite.py [--client DIR] [--out FILE] [--no-names]

  tools/re/py rdb_sqlite.py --text [--out FILE]    text-only copy of an existing full database

Default output: dist/lmo-client.sqlite (Dropbox- and git-ignored: the data is copyrighted).
--text writes dist/lmo-client-text.sqlite from it: the same tables and rows without rdb_body,
BLOB columns (file and field bytes, undecoded/trailing bytes) and checksums.
Nothing is dropped: every resource's body is in `rdb_body`, whether or not we know its format.
Decoded tables are extra views of the same bytes, in the client's own layout and field order.

Sources, first found wins: the client's RDB/<type>/<md5>.rdbres files, then the patch server's
cache (var/rdb-cache) and the Funcom CDN mirror (clients/funcom-cdn/legoupm/rdb), both IOz1
(zlib) wrapped. A .rdbres usually starts with a 12-byte header (u32 type, u32 id | flags, u32 ?),
kept in rdb_resource.header_*; rdb_body.body is what follows (decompressed).

RDBHashIndex.bin ("RDHI", v7): u32 resource count, u32 type count, per type u32 type, u32 n,
n x 47 bytes {u32 id, u32 size, u32 0, md5[16] of the body, u8 flags, u16, md5[16]}; then
u32 n bundles {u32 type, u32 size, md5[16], u32 k, k x u32 id}; then u32 n groups
{u32 len + name (a playfield or "default-resources"), u8, u32 k, k x (u32 type, u32 id)}.

Tables (see the `table_doc` table in the database for every table's description):
  rdb_type, rdb_resource, rdb_body, rdb_name, rdb_bundle(+_member), rdb_group(+_member),
  text_language, text, xml_node, xml_attr, protobuf_field, game_action_list, game_action,
  game_action_criterion, game_action_arg, template, template_stat, simple_dynel,
  playfield_door, minifigure_xp, client_file, decode_error; views resource, text_en.
"""
import argparse, hashlib, json, os, re, sqlite3, struct, sys, time, zlib
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../.."))
TOP = os.path.dirname(ROOT)
sys.path.insert(0, HERE)

STATS = json.load(open(os.path.join(ROOT, "data/stats.json")))
STATS.update(json.load(open(os.path.join(ROOT, "data/stats_linux.json"))))
GA_TYPES = {}
for line in open(os.path.join(ROOT, "data/gameaction_types.txt")):
    p = line.split()
    GA_TYPES[int(p[0])] = (p[2].replace("e_Type_GC_GameAction_", ""), p[3] != "0x0")
TYPES_MD = os.path.join(TOP, "research/lmoxide-notes/client-survey-2/rdb-types.md")
PROTOBUF_TYPES = {1000108, 1000639, 1000641, 1000642, 1000643, 1000644, 1000645, 1000647, 1010223, 1010230}

TABLE_DOC = {
    "rdb_type": "One row per RDB type: resource count and size; loader class, what it holds and status from research/lmoxide-notes/client-survey-2/rdb-types.md (blank where unknown).",
    "rdb_resource": "One row per entry of RDBHashIndex.bin, in index order: every field of the 47-byte index record, the .rdbres header, where the bytes came from and the detected format.",
    "rdb_body": "The body of every resource (decompressed, header removed). Join on (type, id).",
    "rdb_name": "Resource names from the client's name index (RDB 1020004, Patricia trie). certainty: exact (the index rebuilds from the names), lookup (the index resolves the name to this id, but the name may be partly reconstructed), guess.",
    "rdb_bundle": "Bundles listed in RDBHashIndex.bin after the resources: type, size, md5.",
    "rdb_bundle_member": "Resource ids in each bundle, in order.",
    "rdb_group": "Download/preload groups listed at the end of RDBHashIndex.bin: one per playfield plus default-resources.",
    "rdb_group_member": "(type, id) resources of each group, in order.",
    "text_language": "RDB 1030001 TDL1 language maps: language resource id, its header strings, category -> RDB 1030002 id.",
    "text": "Every localized string: from RDB 1030002 TDC2 blobs (source rdb) and Data/text/*/*.tdbc (source file). flags is the TDC2 entry's second u32.",
    "xml_node": "Every element of every XML resource (text XML and LMXB binary XML), with its parent: (type, id, node) identifies it.",
    "xml_attr": "Attributes of xml_node, in document order.",
    "protobuf_field": "Protobuf resources decoded field by field without a schema (wire format only). Length-delimited fields that parse as a message are expanded (children point at parent_field); bytes holds the raw value, text its UTF-8 reading when it is text.",
    "game_action_list": "GameAction lists of item templates (RDB 1000620, RDBItemData_c) and spell templates (1000621, RDBSpellTemplate_c): record version and dynel type, then one row per list (event, u32, count).",
    "game_action": "Each GameActionData: type id and name (data/gameaction_types.txt), whether the client implements it, the two unknown u32s.",
    "game_action_criterion": "The criteria triples (3 x u32) of each game action.",
    "game_action_arg": "The (stat, kind, value) arguments of each game action; stat names from data/stats*.json.",
    "template": "Item (RDB 1000620) and spell (1000621) templates, one layout: u32 version, u32 dynel type, game action lists (game_action_* tables), u32 a, [undecoded bytes when a != 0], u32 c, stat block (template_stat), u16 name, u16 description, [trailing bytes].",
    "template_stat": "Stat block of each template: stat id and name (data/stats*.json), kind (0 int, 1 float, 2 string), value.",
    "simple_dynel": "PlayfieldSimpleDynels_c (RDB 1000057), every entry of every playfield.",
    "playfield_door": "PlayfieldDoors_c (RDB 1000102), every door of every playfield.",
    "minifigure_xp": "MinifigureXPTables (RDB 1000525): the two header u32s and every (level, xp) pair.",
    "client_file": "Every file of the client's Data/ folder (Lua, XML, SWF, shaders, text databases ...), with its bytes and, for text files, its text.",
    "decode_error": "Resources a decoder could not read (the bytes are still in rdb_body).",
}


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


# ---------------------------------------------------------------- sources

def scan_sources(client):
    roots = [os.path.join(client, "RDB"), os.path.join(TOP, "clients/windows/LEGO Minifigures Online/RDB"),
             os.path.join(TOP, "var/rdb-cache"), os.path.join(TOP, "clients/funcom-cdn/legoupm/rdb")]
    files = {}
    for r in roots:
        for d, _, fs in os.walk(r):
            for f in fs:
                p = os.path.join(d, f)
                stem = f.split(".")[0].lower()
                tail = os.path.relpath(p, r).replace(os.sep, "").split(".")[0].lower()[-32:]
                for key in (stem, tail):
                    if len(key) == 32 and re.fullmatch(r"[0-9a-f]{32}", key):
                        files.setdefault(key, p)
    return files


def read_index(path):
    d = open(path, "rb").read()
    assert d[:4] == b"RDHI", d[:4]
    version, nres, ntypes = struct.unpack_from("<3I", d, 4)
    o, res = 16, []
    for _ in range(ntypes):
        t, n = struct.unpack_from("<2I", d, o)
        o += 8
        for _ in range(n):
            rid, size, zero = struct.unpack_from("<3I", d, o)
            res.append(dict(type=t, id=rid, size=size, index_u32=zero, md5=d[o + 12:o + 28].hex(),
                            flags=d[o + 28], index_u16=struct.unpack_from("<H", d, o + 29)[0],
                            index_md5=d[o + 31:o + 47].hex()))
            o += 47
    bundles = []
    n = u32(d, o); o += 4
    for _ in range(n):
        t, size = struct.unpack_from("<2I", d, o)
        h = d[o + 8:o + 24].hex()
        k = u32(d, o + 24); o += 28
        bundles.append((t, size, h, struct.unpack_from(f"<{k}I", d, o)))
        o += 4 * k
    groups = []
    n = u32(d, o); o += 4
    for _ in range(n):
        ln = u32(d, o)
        name = d[o + 4:o + 4 + ln].decode("latin1"); o += 4 + ln
        flag, k = d[o], u32(d, o + 1); o += 5
        pairs = struct.unpack_from(f"<{2 * k}I", d, o); o += 8 * k
        groups.append((name, flag, list(zip(pairs[::2], pairs[1::2]))))
    assert o == len(d), (o, len(d))
    return dict(version=version, count=nres), res, bundles, groups


def load_body(r, path):
    raw = open(path, "rb").read()
    stored = "plain"
    b = raw
    if b[:4] == b"IOz1":
        b = zlib.decompress(b[8:])
        stored = "IOz1"
    hdr = (None, None, None)
    if len(b) >= 12 and u32(b, 0) == r["type"]:
        hdr = struct.unpack_from("<3I", b, 0)
        b = b[12:]
    return b, stored, hdr


MAGICS = [(b"<?xml", "xml"), (b"<", "xml"), (b"LMXB", "lmxb"), (b"TDC2", "tdc2"), (b"TDL1", "tdl1"),
          (b"\x89PNG", "png"), (b"\xff\xd8\xff", "jpeg"), (b"OggS", "ogg"), (b"CWS", "swf"), (b"FWS", "swf"),
          (b"XAC ", "emfx_actor"), (b"XSM ", "emfx_motion"), (b"XBSF", "emfx_blend"), (b"MOS ", "emfx_motionset"),
          (b"SEBD", "physx"), (b"\x1bLua", "lua_bytecode")]


def detect(t, b):
    if t in PROTOBUF_TYPES:
        return "protobuf"
    for m, f in MAGICS:
        if b.startswith(m):
            if f == "xml":
                try:
                    b[:200].decode("utf-8")
                except UnicodeDecodeError:
                    continue
            return f
    return "binary" if b else "empty"


# ---------------------------------------------------------------- decoders

def lmxb(b):
    d = b[8:]
    text = lambda o: d[o:d.index(b"\0", o)].decode("utf-8", "replace")

    def node(o):
        children, attrs, name, nc, na = struct.unpack_from("<3I2H", d, o)
        a = [(text(k), text(v)) for k, v in (struct.unpack_from("<2I", d, attrs + 8 * i) for i in range(na))]
        return text(name), a, None, [node(children + 16 * i) for i in range(nc)]

    return node(u32(d, 0))


def xml_tree(b, recoded=False):
    """-> list of root elements (some server XML holds several documents back to back)."""
    b = b.rstrip(b"\0")

    def node(e):
        txt = (e.text or "").strip() or None
        return e.tag, list(e.attrib.items()), txt, [node(c) for c in e]

    try:
        return [node(ET.fromstring(b))]
    except ET.ParseError as e:
        if "invalid token" in str(e) and not recoded:  # a stray Latin-1 byte (model material names)
            b = re.sub(rb"<\?xml[^>]*\?>", b"", b).decode("latin1").encode("utf-8")
            return xml_tree(b, True)
        if "junk after document element" not in str(e):
            raise
    body = re.sub(rb"<\?xml[^>]*\?>", b"", b)
    return [node(c) for c in ET.fromstring(b"<_>" + body + b"</_>")]


def flatten_xml(roots, t, rid, nodes, attrs):
    stack = [(r, None, 0) for r in reversed(roots)]
    n = 0
    while stack:
        (tag, a, txt, children), parent, depth = stack.pop()
        me = n
        n += 1
        nodes.append((t, rid, me, parent, depth, tag, txt))
        for i, (k, v) in enumerate(a):
            attrs.append((t, rid, me, i, k, v))
        for c in reversed(children):
            stack.append((c, me, depth + 1))


def varint(b, o):
    v = s = 0
    while True:
        if o >= len(b):
            raise ValueError("truncated varint")
        c = b[o]; o += 1
        v |= (c & 0x7F) << s; s += 7
        if not c & 0x80:
            return v, o
        if s > 70:
            raise ValueError("varint too long")


def pb_parse(b):
    out, o = [], 0
    while o < len(b):
        key, o = varint(b, o)
        num, wt = key >> 3, key & 7
        if num == 0:
            raise ValueError("field 0")
        if wt == 0:
            v, o = varint(b, o); out.append((num, wt, v, None))
        elif wt == 1:
            if o + 8 > len(b): raise ValueError("truncated")
            out.append((num, wt, struct.unpack_from("<Q", b, o)[0], b[o:o + 8])); o += 8
        elif wt == 5:
            if o + 4 > len(b): raise ValueError("truncated")
            out.append((num, wt, u32(b, o), b[o:o + 4])); o += 4
        elif wt == 2:
            n, o = varint(b, o)
            if o + n > len(b): raise ValueError("truncated")
            out.append((num, wt, None, b[o:o + n])); o += n
        else:
            raise ValueError(f"wire type {wt}")
    return out


def as_text(v):
    try:
        s = v.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if s and all(c.isprintable() or c in "\r\n\t" for c in s):
        return s
    return None


def pb_rows(b, t, rid, rows, counter, parent=None, path="", depth=0):
    for i, (num, wt, iv, raw) in enumerate(pb_parse(b)):
        me = counter[0]; counter[0] += 1
        p = f"{path}.{num}" if path else str(num)
        txt = sub = None
        fl = None
        if wt == 5:
            fl = struct.unpack("<f", raw)[0]
        elif wt == 1:
            fl = struct.unpack("<d", raw)[0]
        if wt == 2:
            txt = as_text(raw)
            if txt is None and raw and depth < 12:
                try:
                    pb_parse(raw); sub = raw
                except ValueError:
                    pass
        if iv is not None and iv >= 1 << 63:
            iv -= 1 << 64  # stored as signed 64-bit (protobuf's encoding of negative int64)
        rows.append((t, rid, me, parent, p, i, num, wt, iv, fl, raw if sub is None else None, txt, sub is not None))
        if sub is not None:
            pb_rows(sub, t, rid, rows, counter, me, p, depth + 1)


def ga_action(b, o):
    t, a, c, ncrit = struct.unpack_from("<4I", b, o)
    if ncrit > 20:
        return None
    crit = [struct.unpack_from("<3I", b, o + 16 + 12 * i) for i in range(ncrit)]
    p = o + 16 + 12 * ncrit
    n = u32(b, p); p += 4
    if n > 64:
        return None
    args = []
    for _ in range(n):
        k, kind = u32(b, p), b[p + 4]; p += 5
        if kind == 0:
            v = struct.unpack_from("<i", b, p)[0]; p += 4
        elif kind == 1:
            v = struct.unpack_from("<f", b, p)[0]; p += 4
        elif kind == 2:
            ln = u32(b, p); v = b[p + 4:p + 4 + ln].decode("latin1"); p += 4 + ln
        else:
            return None
        args.append((k, kind, v))
    return (t, a, c, crit, args), p


def ga_lists(b, o, n):
    out = []
    for _ in range(n):
        event, x, count = struct.unpack_from("<3I", b, o); o += 12
        acts = []
        for _ in range(count):
            r = ga_action(b, o)
            if not r:
                raise ValueError(f"bad game action at {o}")
            acts.append(r[0]); o = r[1]
        out.append((event, x, acts))
    return out, o


class R:
    def __init__(self, b): self.b, self.o = b, 0
    def u8(self): self.o += 1; return self.b[self.o - 1]
    def u32(self): self.o += 4; return u32(self.b, self.o - 4)
    def i32(self): self.o += 4; return struct.unpack_from("<i", self.b, self.o - 4)[0]
    def f(self): self.o += 4; return struct.unpack_from("<f", self.b, self.o - 4)[0]
    def fs(self, n): self.o += 4 * n; return list(struct.unpack_from(f"<{n}f", self.b, self.o - 4 * n))
    def str(self):
        n = struct.unpack_from("<H", self.b, self.o)[0]; self.o += 2 + n
        return self.b[self.o - n:self.o].decode("latin1")


def simple_dynels(b):
    r = R(b)
    out = []
    for i in range(r.u32()):
        v = r.u32()
        e = [i, v, r.str(), *r.fs(3), *r.fs(4), r.f(), r.i32(), r.u32(), r.u32()]
        e.append(json.dumps(r.fs(6)) if v >= 2 else None)
        e.append(r.u32() if v >= 3 else None)
        if v >= 4:
            e += [r.u8(), r.str()]
        else:
            e += [None, None]
        out.append(e)
    if r.o != len(b):
        raise ValueError(f"{len(b) - r.o} bytes left")
    return out


def doors(b):
    r = R(b)
    version = r.u32()
    out = []
    for i in range(r.u32()):
        e = [i, version, r.str(), r.u32(), r.str(), r.u8(), r.i32(), r.f(), *r.fs(3), *r.fs(4), json.dumps(r.fs(6)), r.u32(), r.u32()]
        out.append(e)
    if r.o != len(b):
        raise ValueError(f"{len(b) - r.o} bytes left")
    return out


def tdc2(b):
    c, zero, size, n = struct.unpack_from("<4I", b, 4)
    strings = b[36:36 + size]
    out = []
    for k in range(n):
        i, flags, o, ln = struct.unpack_from("<4I", b, 36 + size + 16 * k)
        out.append((i, flags, strings[o:o + ln].rstrip(b"\0").decode("utf-8", "replace")))
    return c, zero, b[20:36].hex(), out


def tdl1(b):
    o = 8
    hdr = b[o:o + 3]; o += 3
    strs = []
    for _ in range(3):
        n = u32(b, o)
        strs.append(b[o + 4:o + 4 + n].decode("latin1")); o += 4 + n + 1
    n = u32(b, o); o += 4
    m = [struct.unpack_from("<2I", b, o + 8 * i) for i in range(n)]
    return hdr.hex(), strs, m, b[:8].hex()


def type_notes():
    out = {}
    if not os.path.exists(TYPES_MD):
        return out
    for line in open(TYPES_MD):
        m = re.match(r"^\| (\d{7}(?:[-–/ ,0-9]*)?) \|(.*)\|\s*$", line)
        if not m:
            continue
        cells = [c.strip() for c in m.group(2).split("|")]
        if len(cells) < 5:
            continue
        for t in re.findall(r"\d{7}", m.group(1)):
            out.setdefault(int(t), dict(loader=cells[2], what=cells[3], status=cells[4]))
    return out


# ---------------------------------------------------------------- build

SCHEMA = """
CREATE TABLE rdb_type (type INTEGER PRIMARY KEY, resources INTEGER, total_size INTEGER, formats TEXT, loader TEXT, what TEXT, status TEXT);
CREATE TABLE rdb_resource (type INTEGER REFERENCES rdb_type(type), id INTEGER, index_order INTEGER, size INTEGER, md5 TEXT,
  index_u32 INTEGER, flags INTEGER, index_u16 INTEGER, index_md5 TEXT,
  header_type INTEGER, header_id INTEGER, header_u32 INTEGER, stored TEXT, source TEXT, format TEXT, body_size INTEGER, md5_ok INTEGER,
  PRIMARY KEY (type, id));
CREATE TABLE rdb_body (type INTEGER, id INTEGER, body BLOB, PRIMARY KEY (type, id), FOREIGN KEY (type, id) REFERENCES rdb_resource(type, id));
CREATE TABLE rdb_name (type INTEGER, id INTEGER, name TEXT, certainty TEXT, FOREIGN KEY (type, id) REFERENCES rdb_resource(type, id));
CREATE TABLE rdb_bundle (bundle INTEGER PRIMARY KEY, type INTEGER, size INTEGER, md5 TEXT, members INTEGER);
CREATE TABLE rdb_bundle_member (bundle INTEGER REFERENCES rdb_bundle(bundle), idx INTEGER, type INTEGER, id INTEGER);
CREATE TABLE rdb_group (grp INTEGER PRIMARY KEY, name TEXT, flag INTEGER, members INTEGER);
CREATE TABLE rdb_group_member (grp INTEGER REFERENCES rdb_group(grp), idx INTEGER, type INTEGER, id INTEGER);
CREATE TABLE text_language (language INTEGER, header TEXT, prefix TEXT, string1 TEXT, string2 TEXT, string3 TEXT, category INTEGER, tdc_id INTEGER);
CREATE TABLE text (source TEXT, language TEXT, category INTEGER, id INTEGER, flags INTEGER, text TEXT, tdc_id INTEGER, file TEXT);
CREATE TABLE xml_node (type INTEGER, id INTEGER, node INTEGER, parent INTEGER, depth INTEGER, tag TEXT, text TEXT, PRIMARY KEY (type, id, node));
CREATE TABLE xml_attr (type INTEGER, id INTEGER, node INTEGER, idx INTEGER, name TEXT, value TEXT);
CREATE TABLE protobuf_field (type INTEGER, id INTEGER, field INTEGER, parent_field INTEGER, path TEXT, idx INTEGER, number INTEGER,
  wire_type INTEGER, int_value INTEGER, float_value REAL, bytes BLOB, text TEXT, is_message INTEGER, PRIMARY KEY (type, id, field));
CREATE TABLE game_action_list (type INTEGER, id INTEGER, list INTEGER, version INTEGER, header_u32 INTEGER, event INTEGER, list_u32 INTEGER, actions INTEGER);
CREATE TABLE game_action (type INTEGER, id INTEGER, list INTEGER, action INTEGER, action_type INTEGER, action_name TEXT, client_side INTEGER, u32_a INTEGER, u32_c INTEGER);
CREATE TABLE game_action_criterion (type INTEGER, id INTEGER, list INTEGER, action INTEGER, idx INTEGER, a INTEGER, b INTEGER, c INTEGER);
CREATE TABLE game_action_arg (type INTEGER, id INTEGER, list INTEGER, action INTEGER, idx INTEGER, stat INTEGER, stat_name TEXT, kind INTEGER, value);
CREATE TABLE template (type INTEGER, id INTEGER, version INTEGER, dynel_type INTEGER, lists INTEGER, u32_a INTEGER, undecoded BLOB,
  u32_c INTEGER, name TEXT, description TEXT, trailing BLOB, PRIMARY KEY (type, id));
CREATE TABLE template_stat (type INTEGER, id INTEGER, idx INTEGER, stat INTEGER, stat_name TEXT, kind INTEGER, value);
CREATE TABLE simple_dynel (playfield INTEGER, idx INTEGER, version INTEGER, name TEXT, x REAL, y REAL, z REAL, qx REAL, qy REAL, qz REAL, qw REAL,
  scale REAL, script_group INTEGER, visual_type INTEGER, visual_id INTEGER, bounds TEXT, guid INTEGER, u8 INTEGER, tag TEXT);
CREATE TABLE playfield_door (playfield INTEGER, idx INTEGER, version INTEGER, name TEXT, guid INTEGER, tag TEXT, u8 INTEGER, script_group INTEGER, scale REAL,
  x REAL, y REAL, z REAL, qx REAL, qy REAL, qz REAL, qw REAL, bounds TEXT, visual_type INTEGER, visual_id INTEGER);
CREATE TABLE minifigure_xp (id INTEGER, header_a INTEGER, header_b INTEGER, idx INTEGER, level INTEGER, xp INTEGER);
CREATE TABLE client_file (path TEXT PRIMARY KEY, ext TEXT, size INTEGER, text TEXT, body BLOB);
CREATE TABLE decode_error (type INTEGER, id INTEGER, decoder TEXT, error TEXT);
CREATE TABLE table_doc (name TEXT PRIMARY KEY, description TEXT);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
"""

VIEWS = """
CREATE INDEX rdb_name_by_id ON rdb_name(type, id);
CREATE INDEX rdb_name_by_name ON rdb_name(name);
CREATE INDEX rdb_resource_by_id ON rdb_resource(id);
CREATE INDEX bundle_member_by_res ON rdb_bundle_member(type, id);
CREATE INDEX group_member_by_res ON rdb_group_member(type, id);
CREATE INDEX text_by_id ON text(category, id);
CREATE INDEX xml_attr_by_node ON xml_attr(type, id, node);
CREATE INDEX xml_attr_by_name ON xml_attr(name, value);
CREATE INDEX xml_node_by_tag ON xml_node(tag);
CREATE INDEX game_action_by_res ON game_action(type, id, list, action);
CREATE INDEX game_action_arg_by_res ON game_action_arg(type, id, list, action);
CREATE INDEX game_action_crit_by_res ON game_action_criterion(type, id, list, action);
CREATE INDEX template_stat_by_id ON template_stat(type, id);
CREATE INDEX template_by_name ON template(name);
CREATE VIEW resource AS
  SELECT r.type, t.loader, t.what, r.id, (SELECT name FROM rdb_name n WHERE n.type = r.type AND n.id = r.id) AS name,
         r.format, r.size, r.flags, r.stored, r.source
  FROM rdb_resource r LEFT JOIN rdb_type t ON t.type = r.type;
CREATE VIEW text_en AS SELECT category, id, flags, text FROM text WHERE source = 'rdb' AND language = 'English';
"""


def build(client, out, with_names):
    t0 = time.time()
    log = lambda *a: print(f"[{time.time() - t0:6.0f}s]", *a, file=sys.stderr, flush=True)
    if os.path.exists(out):
        os.unlink(out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    db = sqlite3.connect(out)
    db.executescript("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")
    db.executescript(SCHEMA)
    db.executemany("INSERT INTO table_doc VALUES (?,?)", TABLE_DOC.items())

    head, res, bundles, groups = read_index(os.path.join(client, "RDB/RDBHashIndex.bin"))
    log(f"index: {len(res)} resources, {len(bundles)} bundles, {len(groups)} groups")
    db.executemany("INSERT INTO meta VALUES (?,?)", [
        ("client", os.path.relpath(client, TOP)), ("index_version", head["version"]), ("built", time.strftime("%Y-%m-%d %H:%M:%S")),
        ("client_version", open(os.path.join(client, "Version.txt")).read().strip() if os.path.exists(os.path.join(client, "Version.txt")) else None)])
    for i, (t, size, h, ids) in enumerate(bundles):
        db.execute("INSERT INTO rdb_bundle VALUES (?,?,?,?,?)", (i, t, size, h, len(ids)))
        db.executemany("INSERT INTO rdb_bundle_member VALUES (?,?,?,?)", [(i, k, t, x) for k, x in enumerate(ids)])
    for i, (name, flag, pairs) in enumerate(groups):
        db.execute("INSERT INTO rdb_group VALUES (?,?,?,?)", (i, name, flag, len(pairs)))
        db.executemany("INSERT INTO rdb_group_member VALUES (?,?,?,?)", [(i, k, t, x) for k, (t, x) in enumerate(pairs)])

    files = scan_sources(client)
    log(f"{len(files)} resource files on disk")
    errors, missing = [], 0
    nodes, attrs, pb, texts = [], [], [], []
    gal, gaa, gac, garg = [], [], [], []
    items, istats, dyn, drs, xp = [], [], [], [], []
    langs = []
    formats = {}
    bodies = {}

    def err(t, rid, dec, e):
        errors.append((t, rid, dec, f"{type(e).__name__}: {e}"))

    def flush():
        for sql, rows in (("xml_node", nodes), ("xml_attr", attrs), ("protobuf_field", pb), ("text", texts),
                          ("game_action_list", gal), ("game_action", gaa), ("game_action_criterion", gac),
                          ("game_action_arg", garg)):
            if rows:
                db.executemany(f"INSERT INTO {sql} VALUES ({','.join('?' * len(rows[0]))})", rows)
                rows.clear()

    for n, r in enumerate(res):
        t, rid = r["type"], r["id"]
        path = files.get(r["md5"].lower())
        row = [t, rid, n, r["size"], r["md5"], r["index_u32"], r["flags"], r["index_u16"], r["index_md5"]]
        if not path:
            missing += 1
            db.execute("INSERT INTO rdb_resource VALUES (" + ",".join("?" * 17) + ")", row + [None] * 6 + ["missing", None])
            continue
        try:
            b, stored, hdr = load_body(r, path)
        except Exception as e:  # noqa: BLE001
            err(t, rid, "load", e)
            b, stored, hdr = open(path, "rb").read(), "unreadable", (None, None, None)
        fmt = detect(t, b)
        formats.setdefault(t, set()).add(fmt)
        md5_ok = int(hashlib.md5(b).hexdigest() == r["md5"].lower())
        if not md5_ok and hdr[0] is not None:
            md5_ok = int(hashlib.md5(struct.pack("<3I", *hdr) + b).hexdigest() == r["md5"].lower())
        db.execute("INSERT INTO rdb_resource VALUES (" + ",".join("?" * 17) + ")",
                   row + [*hdr, stored, os.path.relpath(path, TOP), fmt, len(b), md5_ok])
        db.execute("INSERT INTO rdb_body VALUES (?,?,?)", (t, rid, b))
        try:
            if fmt == "xml":
                flatten_xml(xml_tree(b), t, rid, nodes, attrs)
            elif fmt == "lmxb":
                flatten_xml([lmxb(b)], t, rid, nodes, attrs)
            elif fmt == "protobuf":
                pb_rows(b, t, rid, pb, [0])
            elif fmt == "tdc2":
                c, _z, _h, entries = tdc2(b)
                bodies[(t, rid)] = c
                texts.extend(("rdb", None, c, i, fl, s, rid, None) for i, fl, s in entries)
            elif fmt == "tdl1":
                hdr3, strs, m, pre = tdl1(b)
                langs.extend((rid, hdr3, pre, *strs, c, i) for c, i in m)
            elif t in (1000620, 1000621):
                version, dtype, nl = struct.unpack_from("<3I", b, 0)
                lists, end = ga_lists(b, 12, nl)
                for li, (event, x, acts) in enumerate(lists):
                    gal.append((t, rid, li, version, dtype, event, x, len(acts)))
                    for ai, (at, a, c, crit, args) in enumerate(acts):
                        name, cl = GA_TYPES.get(at, (None, None))
                        gaa.append((t, rid, li, ai, at, name, cl, a, c))
                        gac.extend((t, rid, li, ai, k, *cr) for k, cr in enumerate(crit))
                        garg.extend((t, rid, li, ai, k, s, STATS.get(str(s)), kind, v) for k, (s, kind, v) in enumerate(args))
                ua, extra, uc, stats, name, desc, rest = template_tail(b, end)
                items.append((t, rid, version, dtype, nl, ua, extra or None, uc, name, desc, rest or None))
                istats.extend((t, rid, i, st, STATS.get(str(st)), kind, v) for i, (st, kind, v) in enumerate(stats))
            elif t == 1000057:
                dyn.extend([rid, *e] for e in simple_dynels(b))
            elif t == 1000102:
                drs.extend([rid, *e] for e in doors(b))
            elif t == 1000525:
                a, c = struct.unpack_from("<2I", b, 0)
                xp.extend((rid, a, c, i, *struct.unpack_from("<2I", b, 8 + 8 * i)) for i in range((len(b) - 8) // 8))
        except Exception as e:  # noqa: BLE001
            err(t, rid, fmt if fmt in ("xml", "lmxb", "protobuf", "tdc2", "tdl1") else str(t), e)
        if n % 2000 == 0:
            flush()
            log(f"{n}/{len(res)}")
    flush()
    db.executemany("INSERT INTO text_language VALUES (?,?,?,?,?,?,?,?)", langs)
    # language codes of the RDB texts, from the TDL1 maps
    lang_of = {}
    for lid, _h, _p, s1, s2, s3, c, i in langs:
        code = next((s for s in (s1, s2, s3) if re.fullmatch(r"[a-z]{2}", s or "")), s1)
        lang_of[i] = code
    db.executemany("UPDATE text SET language = ? WHERE source = 'rdb' AND tdc_id = ?", [(v, k) for k, v in lang_of.items()])
    db.executemany("INSERT INTO template VALUES (?,?,?,?,?,?,?,?,?,?,?)", items)
    db.executemany("INSERT INTO template_stat VALUES (?,?,?,?,?,?,?)", istats)
    db.executemany("INSERT INTO simple_dynel VALUES (" + ",".join("?" * 19) + ")", dyn)
    db.executemany("INSERT INTO playfield_door VALUES (" + ",".join("?" * 19) + ")", drs)
    db.executemany("INSERT INTO minifigure_xp VALUES (?,?,?,?,?,?)", xp)
    log(f"bodies done, {missing} missing, {len(errors)} decode errors")

    notes = type_notes()
    by_type = {}
    for r in res:
        s = by_type.setdefault(r["type"], [0, 0])
        s[0] += 1; s[1] += r["size"]
    db.executemany("INSERT INTO rdb_type VALUES (?,?,?,?,?,?,?)", [
        (t, n, size, ",".join(sorted(formats.get(t, ()))), notes.get(t, {}).get("loader"), notes.get(t, {}).get("what"),
         notes.get(t, {}).get("status")) for t, (n, size) in sorted(by_type.items())])

    # loose Data/ files
    data = os.path.join(client, "Data")
    for d, _, fs in os.walk(data):
        for f in sorted(fs):
            p = os.path.join(d, f)
            rel = os.path.relpath(p, client)
            b = open(p, "rb").read()
            ext = f.rsplit(".", 1)[-1].lower() if "." in f else ""
            txt = None
            if ext not in ("swf", "png", "jpg", "dds", "tga", "cur", "ico", "texture", "tdbc", "tdbl", "apx"):
                try:
                    txt = b.decode("utf-8")
                except UnicodeDecodeError:
                    txt = None
            db.execute("INSERT INTO client_file VALUES (?,?,?,?,?)", (rel, ext, len(b), txt, b))
            if b[:4] == b"TDC2":
                try:
                    c, _z, _h, entries = tdc2(b)
                    lang = os.path.basename(d)
                    texts.extend(("file", lang, c, i, fl, s, None, rel) for i, fl, s in entries)
                except Exception as e:  # noqa: BLE001
                    errors.append((None, None, "tdc2 " + rel, str(e)))
    flush()
    log("Data/ files done")

    if with_names:
        names(db, log)
    db.executemany("INSERT INTO decode_error VALUES (?,?,?,?)", errors)
    db.executescript(VIEWS)
    db.commit()
    log("analyze")
    db.execute("ANALYZE")
    db.commit()
    db.close()
    log(f"done: {out}")


def template_tail(b, o):
    """After the game action lists of an item/spell template: u32 a (0 almost always), then (when a
    is not 0) bytes not decoded yet, u32 c (1), u32 n, n x (u32 stat, u8 kind 0 int / 1 float /
    2 string, value), u16 len + name, u16 len + description, sometimes bytes after that. The stat
    block is found as the first offset from which the rest parses."""
    ua = u32(b, o)
    for p in range(o + 4, len(b) - 9):
        try:
            q = p
            uc, n = struct.unpack_from("<2I", b, q); q += 8
            if n > 500:
                continue
            stats = []
            for _ in range(n):
                st, kind = struct.unpack_from("<IB", b, q); q += 5
                if kind == 2:
                    ln = u32(b, q); stats.append((st, kind, b[q + 4:q + 4 + ln].decode("latin1"))); q += 4 + ln
                    continue
                if kind not in (0, 1):
                    raise ValueError
                stats.append((st, kind, struct.unpack_from("<f" if kind else "<i", b, q)[0])); q += 4
            ln = struct.unpack_from("<H", b, q)[0]
            name = b[q + 2:q + 2 + ln]; q += 2 + ln
            ln = struct.unpack_from("<H", b, q)[0]
            desc = b[q + 2:q + 2 + ln]; q += 2 + ln
            if q > len(b):
                continue
            # the normal layout (a = 0: the block follows), else the first offset that ends the record
            if (ua == 0 and p == o + 4) or q == len(b):
                return ua, b[o + 4:p], uc, stats, name.decode("latin1"), desc.decode("latin1"), b[q:]
        except (struct.error, ValueError):
            continue
    raise ValueError("no stat block found after the action lists")


def names(db, log):
    import rdb, rdb_names
    complete = {1010002, 1010003, 1010028, 1010029, 1010200, 1010201, 1010207, 1020005, 1050002}
    idx = rdb.index()
    # read bodies from the database being built (rdb.body's cache fallback walks the whole cache)
    rdb.body = lambda t, i, *_a, **_k: db.execute("SELECT body FROM rdb_body WHERE type = ? AND id = ?", (t, i)).fetchone()[0]
    load, loaded = rdb_names.load, {}

    def cached(rtype, idx=None):
        if rtype not in loaded:
            loaded[rtype] = load(rtype)
        return loaded[rtype]

    rdb_names.load = cached
    for (t, rid) in sorted(k for k in idx if k[0] == rdb_names.INDEX):
        try:
            ns = rdb_names.names(rid)
        except Exception as e:  # noqa: BLE001
            db.execute("INSERT INTO decode_error VALUES (?,?,?,?)", (rdb_names.INDEX, rid, "names", f"{type(e).__name__}: {e}"))
            continue
        rows = []
        for i, name in ns.items():
            if rid in complete:
                cert = "exact"
            else:
                cert = "lookup" if rdb_names.lookup(rid, name) == i else "guess"
            rows.append((rid, i, name, cert))
        db.executemany("INSERT INTO rdb_name VALUES (?,?,?,?)", rows)
        log(f"names of {rid}: {len(rows)}")


SLIM_DROP_COLUMNS = {"md5", "index_md5", "md5_ok"}


def slim(full, out):
    """Copy `full` without bodies, BLOB columns and checksums, keeping keys, foreign keys, indexes, views."""
    if os.path.exists(out):
        os.unlink(out)
    db = sqlite3.connect(out)
    db.executescript("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")
    db.execute("ATTACH DATABASE ? AS f", (full,))
    q = lambda n: '"' + n + '"'
    tables = [r[0] for r in db.execute("SELECT name FROM f.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY rowid")]
    kept = {}
    for t in tables:
        if t == "rdb_body":
            continue
        info = db.execute(f"PRAGMA f.table_info({q(t)})").fetchall()  # cid, name, type, notnull, default, pk
        cols = [c for c in info if c[2].upper() != "BLOB" and c[1] not in SLIM_DROP_COLUMNS]
        names = {c[1] for c in cols}
        kept[t] = names
        defs = [f"{q(c[1])} {c[2]}".rstrip() for c in cols]
        pk = [c[1] for c in sorted(info, key=lambda c: c[5]) if c[5]]
        if pk and set(pk) <= names:
            defs.append(f"PRIMARY KEY ({', '.join(map(q, pk))})")
        fks = {}
        for _id, _seq, table, frm, to, *_ in db.execute(f"PRAGMA f.foreign_key_list({q(t)})"):
            fks.setdefault(_id, []).append((table, frm, to))
        for parts in fks.values():
            table = parts[0][0]
            if table == "rdb_body" or not all(frm in names for _, frm, _ in parts):
                continue
            defs.append(f"FOREIGN KEY ({', '.join(q(f) for _, f, _ in parts)}) REFERENCES {q(table)}"
                        f"({', '.join(q(x) for _, _, x in parts)})")
        db.execute(f"CREATE TABLE {q(t)} ({', '.join(defs)})")
        sel = ", ".join(q(c[1]) for c in cols)
        db.execute(f"INSERT INTO {q(t)} SELECT {sel} FROM f.{q(t)}")
    db.execute("UPDATE table_doc SET description = description || ' (Text-only copy: bytes, BLOB columns and checksums left out; the full database has them.)' "
               "WHERE name IN ('rdb_resource', 'protobuf_field', 'template', 'client_file')")
    db.execute("DELETE FROM table_doc WHERE name = 'rdb_body'")
    db.execute("INSERT INTO meta VALUES ('variant', 'text-only')")
    for kind, name, tbl, sql in db.execute("SELECT type, name, tbl_name, sql FROM f.sqlite_master WHERE type IN ('index', 'view') AND sql IS NOT NULL").fetchall():
        if kind == "index":
            cols = re.search(r"\((.*)\)\s*$", sql).group(1)
            if tbl not in kept or not all(c.strip().strip('"') in kept[tbl] for c in cols.split(",")):
                continue
        db.execute(sql)
    db.commit()
    db.execute("DETACH DATABASE f")
    db.execute("ANALYZE")
    db.commit()
    db.execute("VACUUM")
    db.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--client", default=os.path.join(TOP, "clients/linux/LEGO Minifigures Online"))
    ap.add_argument("--out", default=os.path.join(ROOT, "dist/lmo-client.sqlite"))
    ap.add_argument("--no-names", action="store_true", help="skip the (slow) name index reconstruction")
    ap.add_argument("--text", action="store_true", help="write the text-only copy of the full database (--out: its path)")
    a = ap.parse_args()
    if a.text:
        full = os.path.join(ROOT, "dist/lmo-client.sqlite")
        out = a.out if a.out != ap.get_default("out") else os.path.join(ROOT, "dist/lmo-client-text.sqlite")
        slim(full, os.path.abspath(out))
        print(out)
        return
    os.environ.setdefault("LMO_CLIENT", a.client)
    build(os.path.abspath(a.client), os.path.abspath(a.out), not a.no_names)


if __name__ == "__main__":
    main()
