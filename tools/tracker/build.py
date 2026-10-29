#!/usr/bin/env python3
"""Write the verification checklist (docs/tracker/: README.md + one file per section) from
tools/tracker/lists.py.

  python3 tools/tracker/build.py [out-dir]

The existing files are read first: every box keeps its tick by the hidden `<!-- key -->` at the end
of its line, so ticking boxes (in an editor or on GitHub) and re-running is safe. Ticked boxes whose
item is gone are reported and kept in no-longer-listed.md.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import lists  # noqa: E402

BOX = re.compile(r'^\s*- \[([ xX])\] (.*?)\s*<!-- (\S+) -->\s*$')


def read_state(out_dir):
    state = {}
    if os.path.isdir(out_dir):
        for name in sorted(os.listdir(out_dir)):
            if not name.endswith('.md'):
                continue
            with open(os.path.join(out_dir, name)) as fh:
                for line in fh:
                    m = BOX.match(line)
                    if m:
                        state[m.group(3)] = (m.group(1) != ' ', m.group(2))
    return state


def esc(s):
    return str(s).replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


class Doc:
    def __init__(self, state):
        self.state = state
        self.used = set()
        self.lines = []
        self.files = {}  # file name -> lines

    def new_file(self, name):
        self.lines = self.files[name] = []

    def box(self, key, text, depth=0):
        """A checkbox line; returns whether it is ticked."""
        done = self.state.get(key, (False,))[0]
        self.used.add(key)
        self.lines.append(f"{'  ' * depth}- [{'x' if done else ' '}] {text} <!-- {key} -->")
        return done

    def details(self, summary, body):
        """A <details> block; body(doc) adds the lines and returns (done, total)."""
        start = len(self.lines)
        self.lines.append('')  # summary goes here once the counts are known
        self.lines.append('')
        done, total = body()
        self.lines.append('')
        self.lines.append('</details>')
        self.lines.append('')
        self.lines[start] = f'<details><summary>{esc(summary)} ({done}/{total})</summary>'
        return done, total

    def boxes(self, items):
        """items: (key, text, depth) -> (done, total)."""
        done = total = 0
        for key, text, depth in items:
            done += self.box(key, text, depth)
            total += 1
        return done, total


def grouped(rows, field):
    out = {}
    for r in rows:
        out.setdefault(r[field], []).append(r)
    return out


def section(doc, title, note, groups, render):
    """groups: {name: rows}; render(row) -> [(key, text, depth)]. Each section is its own file."""
    fname = re.sub(r'^(\d)\b', r'0\1', re.sub(r'[^a-z0-9]+', '-', title.lower()).strip('-')) + '.md'
    doc.new_file(fname)
    doc.lines += [f'# {title}', '', '[← tracker](README.md)', '']
    if note:
        doc.lines += [note, '']
    head = len(doc.lines)
    doc.lines.append('')
    done = total = 0
    for name, rows in groups.items():
        d, t = doc.details(name, lambda rows=rows: doc.boxes(i for r in rows for i in render(r)))
        done += d
        total += t
    doc.lines[head] = f'**{done}/{total}** checked'
    doc.lines.insert(head + 1, '')
    return title, fname, done, total


def ach_text(r):
    obj = f' · _{esc(r["objective"])}_' if r['objective'] else ''
    desc = f' — {esc(r["description"])}' if r['description'] else ''
    return f'**{esc(r["title"])}**{desc}{obj}'


def build(state):
    doc = Doc(state)
    totals = []

    ups = lists.upgrades()
    totals.append(section(
        doc, '1. Upgrades', 'One box per minifigure that has the upgrade: tick when it behaves exactly as on live.',
        {f'{u["name"]} — {len(u["minifigs"])} minifigures': [u] for u in ups},
        lambda u: [(m['key'], f'{esc(m["minifig"])} (upgrade {m["number"]})', 0) for m in u['minifigs']]))

    chars = lists.characters()
    totals.append(section(
        doc, '2. Minifigures and their abilities', 'Main = normal attack, secondary = special.',
        {f'Series {s}': rows for s, rows in grouped(chars, 'series').items()},
        lambda c: [(c['key'] + '/main', f'{esc(c["name"])} — main: {esc(c["main"])}', 0),
                   (c['key'] + '/secondary', f'{esc(c["name"])} — secondary: {esc(c["secondary"])}', 0)]))

    en = lists.enemies()
    order = sorted(grouped(en, 'group').items(), key=lambda kv: (kv[0] in ('Several worlds', 'Not placed', 'Summons'), kv[0]))
    totals.append(section(
        doc, '3. Enemies', 'Grouped by the world they are spawned in (content spawns); "Not placed" ones come from encounters, scripts or nowhere yet.',
        dict(order),
        lambda e: [(e['key'], f'**{esc(e["name"])}** — level {e["level"]}, health {e["health"]}'
                    + (f' · {esc(", ".join(e["worlds"]))}' if e['worlds'] else ''), 0)]))

    ab = lists.enemy_abilities()
    totals.append(section(
        doc, '4. Enemy abilities', 'Skill names are the client\'s internal ones.',
        {'Basic attacks': [a for a in ab if a['basic_attack']], 'Skills': [a for a in ab if not a['basic_attack']]},
        lambda a: [(a['key'], f'**{esc(a["name"])}** (spell {a["spell"]})'
                    + (' ⚠ no client definition' if a['no_client_definition'] else '')
                    + f' — {esc(", ".join(a["enemies"]))}', 0)]))

    totals.append(section(
        doc, '5. World achievements', None, lists.world_achievements(),
        lambda r: [(r['key'], ach_text(r), r['depth'])]))

    totals.append(section(
        doc, '6. Global achievements', None, {'Global': lists.global_achievements()},
        lambda r: [(r['key'], ach_text(r), r['depth'])]))

    mf = lists.minifig_achievements()
    doc_note = ('Per-minifigure achievements without upgrades, Lost Creations levels and the any/all roll-ups; '
                '`{minifig}` stands for each minifigure listed.')
    totals.append(section(
        doc, '7. Minifigure achievements', doc_note,
        {f'{g["title"]} — {len(g["minifigs"])} minifigures': [g] for g in mf},
        lambda g: [(m['key'], f'{esc(m["minifig"])} — {esc(g["description"].replace("{minifig}", m["minifig"]))} · _{esc(g["objective"])}_', 0)
                   for m in g['minifigs']]))

    q = lists.area_quests()
    totals.append(section(
        doc, '8. Area quests', '⚠ no area = no `Quest_*` area runs it yet.',
        dict(sorted(grouped(q, 'world').items())),
        lambda r: [(r['key'], ach_text(r) + (' ⚠ no area' if not r['has_area'] else ''), 0)]))

    def world_text(w):
        bits = [f'pf {w["playfield"]}', w['kind']]
        if w['level_cap']:
            bits.append(f'level cap {w["level_cap"]}')
        if w['element']:
            bits.append(w['element'])
        if w['minifig']:
            bits.append(f'rewards {w["minifig"]}')
        if w['reached_by'] != 'portal':
            bits.append('accessible per manual.json')
        return f'**{esc(w["name"])}** ({esc(", ".join(bits))})'

    note = 'Accessible in live = reachable through portals from the LEGO Center, plus `live_extra` in tools/tracker/manual.json.'
    totals.append(section(doc, '9. Adventure zones', note, grouped(lists.adventure_zones(), 'region'),
                          lambda w: [(w['key'], world_text(w), 0)]))
    totals.append(section(doc, '10. Pocket adventures', note, grouped(lists.pocket_adventures(), 'region'),
                          lambda w: [(w['key'], world_text(w), 0)]))

    def lc_items(lc):
        out = [(lc['key'], f'**{esc(lc["name"])}** ({lc["mode"]}, pf {lc["playfield"]})', 0)]
        out += [(v['key'], f'variant: {esc(v["name"])}', 1) for v in lc['variants']]
        return out
    lcs = lists.lost_creations()
    missing = [lc['name'] for lc in lcs if not lc['variants']]
    note = ('Variants come from `lost_creations.variants` in tools/tracker/manual.json'
            + (f'; none entered yet for {len(missing)} of {len(lcs)} levels.' if missing else '.'))
    totals.append(section(doc, '11. Lost Creations', note, {'Levels': lcs}, lc_items))

    dl = lists.dialogues()
    story = {w: rows for w, rows in sorted(dl.items(), key=lambda kv: (kv[0] == 'Unassigned', kv[0])) if w != 'other'}

    def dl_items(r):
        trig = f'**triggered** ({r["trigger"]})' if r['triggered'] else 'not triggered'
        ids = ', '.join(map(str, r['texts']))
        return [(r['key'], f'#{ids} “{esc(r["preview"]) or "(no spoken line)"}” · {trig}', 0)]
    note = ('The client\'s dialogue texts (TDB 50002) as conversations, put in worlds by speaker (tools/tracker/manual.json '
            '`dialogues`; fix one with `texts`). Triggered = the server has a dialogue record for it (content/dialogues). '
            'Tick when it plays as on live. Summary counts: ' + ', '.join(
                f'{w} {sum(r["triggered"] for r in rows)}/{len(rows)} triggered' for w, rows in story.items()))
    totals.append(section(doc, '12. Story dialogues', note, story, dl_items))
    totals.append(section(doc, '12b. Other dialogues', 'Tests and texts with no spoken line (camera/script only).',
                          {'Other': dl.get('other', [])}, dl_items))

    gone = [(k, v[1]) for k, v in state.items() if v[0] and k not in doc.used]
    if gone:
        doc.new_file('no-longer-listed.md')
        doc.lines += ['# No longer listed', '', '[← tracker](README.md)', '',
                      'Ticked before, but the item is gone from the lists:', '']
        for k, text in gone:
            doc.box(k, text)
        totals.append(('No longer listed', 'no-longer-listed.md', len(gone), len(gone)))

    readme = ['# LMOxide verification tracker', '',
              'Generated by `python3 tools/tracker/build.py` from the content tree. Tick boxes in these files '
              '(an editor, or GitHub\'s web editor) and re-run: ticks are kept by the hidden key on each line. '
              'Hand-kept input: `tools/tracker/manual.json`. The raw lists: `python3 tools/tracker/lists.py`.', '',
              '| Section | Checked |', '|---|---|']
    readme += [f'| [{t}]({f}) | {d}/{n} |' for t, f, d, n in totals]
    files = {name: '\n'.join(lines).rstrip() + '\n' for name, lines in doc.files.items()}
    files['README.md'] = '\n'.join(readme) + '\n'
    return files, gone


def main(argv):
    out = argv[1] if len(argv) > 1 else os.path.join(lists.ROOT, 'docs', 'tracker')
    state = read_state(out)
    files, gone = build(state)
    os.makedirs(out, exist_ok=True)
    for name in os.listdir(out):
        if name.endswith('.md') and name not in files:
            os.remove(os.path.join(out, name))  # e.g. no-longer-listed.md once empty
    for name, text in files.items():
        with open(os.path.join(out, name), 'w') as fh:
            fh.write(text)
    kept = sum(1 for v in state.values() if v[0])
    boxes = sum(t.count('- [') for t in files.values())
    print(f'{out}: {len(files)} files, {boxes} boxes, {kept} ticks kept'
          + (f', {len(gone)} no longer listed' if gone else ''))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
