#!/usr/bin/env python3
"""The lists behind docs/TRACKER.md, read from the content tree (content/) and tools/tracker/manual.json.

  python3 tools/tracker/lists.py                  # what lists there are
  python3 tools/tracker/lists.py upgrades         # one list as text
  python3 tools/tracker/lists.py upgrades --json  # ... as JSON

Every row has a `key` that stays the same between runs; build.py keeps check marks by it.
"""
import collections
import glob
import json
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
CONTENT = os.path.join(ROOT, 'content')
MANUAL = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'manual.json')


def load_dir(kind):
    out = {}
    for f in sorted(glob.glob(os.path.join(CONTENT, kind, '*.json'))):
        with open(f) as fh:
            r = json.load(fh)
        out[r['id']] = r
    return out


def slug(i):
    return i.split('/', 1)[1] if i and '/' in i else i


class Data:
    """Loads each part of the content tree once, on first use."""

    def __getattr__(self, name):
        loader = getattr(self, '_load_' + name)
        value = loader()
        setattr(self, name, value)
        return value

    def _load_manual(self):
        with open(MANUAL) as fh:
            return json.load(fh)

    def _load_minifigs(self):
        return load_dir('minifigures')

    def _load_buffs(self):
        return load_dir('buffs')

    def _load_npcs(self):
        return load_dir('npcs')

    def _load_skills(self):
        return load_dir('skills')

    def _load_achievements(self):
        return load_dir('achievements')

    def _load_quests(self):
        return load_dir('quests')

    def _load_dialogues(self):
        return load_dir('dialogues')

    def _load_lcs(self):
        return load_dir('lost-creations')

    def _load_regions(self):
        return load_dir('regions')

    def _load_texts(self):
        with open(os.path.join(CONTENT, 'texts.json')) as fh:
            return json.load(fh)

    def _load_worlds(self):
        """world id -> world.json record, plus `_dir` (its folder) and `_portals` (destination ids)."""
        out = {}
        for f in sorted(glob.glob(os.path.join(CONTENT, 'worlds', '*', 'world.json'))):
            d = os.path.dirname(f)
            with open(f) as fh:
                r = json.load(fh)
            r['_dir'] = d
            r['_portals'] = []
            p = os.path.join(d, 'portals.json')
            if os.path.exists(p):
                with open(p) as fh:
                    r['_portals'] = [x['to']['world'] for x in json.load(fh)['portals'] if x.get('to', {}).get('world')]
            out[r['id']] = r
        return out

    def _load_spawn_worlds(self):
        """npc id -> world ids it is spawned in."""
        out = collections.defaultdict(set)
        for wid, w in self.worlds.items():
            p = os.path.join(w['_dir'], 'spawns.json')
            if not os.path.exists(p):
                continue
            with open(p) as fh:
                for s in json.load(fh).get('spawns', []):
                    if s.get('template'):
                        out[s['template']].add(wid)
        return out

    def _load_stat_names(self):
        names = {}
        for f in ('stats.json', 'stats_linux.json'):
            p = os.path.join(ROOT, 'data', f)
            if os.path.exists(p):
                with open(p) as fh:
                    names.update(json.load(fh))
        return names

    def world_name(self, wid):
        w = self.worlds.get(wid)
        return w['name'] if w else slug(wid)

    def region_name(self, rid):
        r = self.regions.get(rid)
        return r.get('name', slug(rid)) if r else (slug(rid) or '')


D = Data()


def minifig_order(m):
    return (m.get('series', 99), m.get('series_index', 99), m['name'])


# ---- 1. upgrades ----------------------------------------------------------------------------

def upgrades():
    """Every upgrade (by name) and every minifigure that has it, in the minifigure's upgrade order."""
    by_name = collections.OrderedDict()
    for m in sorted(D.minifigs.values(), key=minifig_order):
        for u in m.get('upgrades', []):
            buff = D.buffs.get(u['buff'], {})
            name = re.sub(r'^Upgrade: ', '', buff.get('name', slug(u['buff'])))
            if name == 'No upgrade available':
                continue
            by_name.setdefault(name, []).append({
                'key': f"upgrade/{slug(u['buff'])}/{slug(m['id'])}",
                'minifig': m['name'],
                'buff': u['buff'],
                'number': u['bit'] + 1,
            })
    return [{'key': 'upgrade/' + re.sub(r'\W+', '-', n.lower()).strip('-'), 'name': n, 'minifigs': v}
            for n, v in sorted(by_name.items())]


# ---- 2. characters --------------------------------------------------------------------------

def characters():
    """Every minifigure with its main (normal) and secondary (special) ability."""
    out = []
    for m in sorted(D.minifigs.values(), key=minifig_order):
        out.append({
            'key': 'minifig/' + slug(m['id']),
            'name': m['name'],
            'series': m.get('series'),
            'main': (m.get('normal') or {}).get('name', '?'),
            'secondary': (m.get('special') or {}).get('name', '?'),
        })
    return out


# ---- 3./4. enemies and their abilities ------------------------------------------------------

def enemy_npcs():
    return [n for n in D.npcs.values() if n['kind'] in ('enemy', 'boss', 'champion', 'summon')]


def enemy_region(n):
    regions = sorted({D.region_name(D.worlds[w].get('region')) for w in D.spawn_worlds.get(n['id'], ())
                      if D.worlds[w].get('region')})
    return regions[0] if len(regions) == 1 else ('Several worlds' if regions else 'Not placed')


def enemies():
    """Every enemy (content npcs of kind enemy/boss/champion/summon) and where it is spawned."""
    out = []
    for n in sorted(enemy_npcs(), key=lambda n: n['name']):
        out.append({
            'key': 'enemy/' + slug(n['id']),
            'name': n['name'].strip(),
            'kind': n['kind'],
            'level': n.get('level'),
            'health': (n.get('stats') or {}).get('health'),
            'group': 'Summons' if n['kind'] == 'summon' else enemy_region(n),
            'worlds': sorted(D.world_name(w) for w in D.spawn_worlds.get(n['id'], ())),
        })
    return out


def enemy_abilities():
    """Every skill an enemy uses (its basic attack and its skill list) and the enemies using it."""
    users = collections.defaultdict(list)
    basic = set()
    for n in enemy_npcs():
        if n.get('attack'):
            users[n['attack']].append(n['name'])
            basic.add(n['attack'])
        for s in n.get('skills', []):
            users[s['skill']].append(n['name'])
    out = []
    for sid, names in users.items():
        sk = D.skills.get(sid, {})
        out.append({
            'key': 'enemy-skill/' + slug(sid),
            'name': sk.get('name', slug(sid)),
            'spell': (sk.get('client') or {}).get('spell'),
            'basic_attack': sid in basic,
            'no_client_definition': 'no-client-definition' in sk.get('tags', []),
            'enemies': sorted(set(names)),
        })
    return sorted(out, key=lambda r: r['name'])


# ---- 5.-7. achievements ---------------------------------------------------------------------

WORLD_CATEGORIES = ['Pirate World', 'Medieval World', 'Space World', 'Mythology World', 'Dino World']
MINIFIG_SKIP_KINDS = {'upgrade', 'lost_creations', 'any', 'all'}


def objective(a):
    k = a['kind']
    if k in ('target', 'defeat', 'clear'):
        return f"{k}: {a.get('target', '?')}"
    if k == 'counter':
        stat, n = a['counter']
        return f"counter: {n} (stat {stat})"
    if k == 'find':
        where = f" in {D.world_name(a['world'])}" if a.get('world') else ''
        return f"find: {a.get('place') or a.get('find')} ({a.get('find')}){where}"
    if k == 'all':
        return f"all {len(a.get('children', []))} below"
    if k == 'any':
        return f"any {a.get('n')} of the {a.get('minifigure') and D.minifigs[a['minifigure']]['name'] or ''} achievements"
    if k in ('upgrade', 'lost_creations'):
        return f"{k}: {a.get('n')}"
    if k == 'series':
        return 'series: every minifigure mastered'
    if k == 'manual':
        return 'manual: awarded by a server event'
    return k


def achievement_row(a):
    return {
        'key': 'ach/' + slug(a['id']),
        'title': a.get('title') or a['client_name'],
        'description': a.get('text', ''),
        'objective': objective(a),
        'kind': a['kind'],
        'points': a.get('points'),
    }


def hidden(a):
    return a.get('invisible') or (a.get('title') or a['client_name']).startswith('XX_')


def achievement_tree(root_id):
    """Rows under root_id in tree order with `depth`; untitled sub-categories are flattened."""
    kids = collections.defaultdict(list)
    for a in D.achievements.values():
        kids[a.get('parent')].append(a)
    out = []

    def walk(pid, depth):
        for a in sorted(kids[pid], key=lambda a: (a.get('title') is None, a.get('title') or a['client_name'])):
            if hidden(a):
                continue
            if a['kind'] == 'category' and not a.get('title'):
                walk(a['id'], depth)
                continue
            out.append(dict(achievement_row(a), depth=depth))
            walk(a['id'], depth + 1)
    walk(root_id, 0)
    return out


def world_achievements():
    """{world category: rows in tree order}."""
    return {c: achievement_tree('achievement/' + re.sub(r'\W+', '-', c.lower())) for c in WORLD_CATEGORIES}


def global_achievements():
    return achievement_tree('achievement/global')


def minifig_achievements():
    """Per-minifigure achievements (series categories) except upgrades, Lost Creations levels and
    the any/all roll-ups, grouped by their wording with the minifigure name taken out."""
    groups = collections.OrderedDict()
    for a in sorted(D.achievements.values(), key=lambda a: a.get('title') or ''):
        if not a.get('minifigure') or a['kind'] in MINIFIG_SKIP_KINDS or hidden(a):
            continue
        m = D.minifigs.get(a['minifigure'])
        if not m:
            continue
        name = m['name']
        title = (a.get('title') or '').replace(name, '{minifig}')
        text = a.get('text', '').replace(name, '{minifig}')
        g = groups.setdefault((title, text), {
            'key': 'mfach/' + re.sub(r'\W+', '-', title.replace('{minifig}', 'x').lower()).strip('-'),
            'title': title, 'description': text, 'objective': objective(a), 'minifigs': []})
        g['minifigs'].append({'key': 'ach/' + slug(a['id']), 'minifig': name})
    return sorted(groups.values(), key=lambda g: (-len(g['minifigs']), g['title']))


# ---- 8. area quests -------------------------------------------------------------------------

def quest_objective(goal):
    bits = [goal.get('kind', '?')]
    if goal.get('counts'):
        bits.append(goal['counts'])
    if goal.get('count'):
        bits.append(f"× {goal['count']}")
    if goal.get('enemies'):
        bits.append('(' + ', '.join(goal['enemies']) + ')')
    return ' '.join(bits)


def area_quests():
    out = []
    for q in sorted(D.quests.values(), key=lambda q: q['client_name']):
        if q['kind'] != 'area':
            continue
        steps = q.get('steps', [])
        goals = [g for s in steps for g in s.get('goals', [])]
        out.append({
            'key': 'quest/' + slug(q['id']),
            'title': q['name'],
            'client_name': q['client_name'],
            'world': D.world_name(q['world']) if q.get('world') else 'No world',
            'description': ' / '.join(s['description'] for s in steps if s.get('description'))
                           or ' / '.join(g.get('text', '') for g in goals),
            'objective': '; '.join(filter(None, (g.get('text', '') + (' — ' if g.get('text') else '') + quest_objective(g)
                                                 for g in goals))),
            'has_area': bool(q.get('area')),
            'temp': q['client_name'].lower().startswith(('[temp', 'temp')),
        })
    return out


# ---- 9./10. adventure zones and pocket adventures --------------------------------------------

def live_worlds():
    m = D.manual['worlds']
    seen = {'world/lego-center'}
    todo = list(seen)
    while todo:
        for t in D.worlds.get(todo.pop(), {}).get('_portals', []):
            if t not in seen:
                seen.add(t)
                todo.append(t)
    seen |= {'world/' + s for s in m.get('live_extra', [])}
    return seen - {'world/' + s for s in m.get('exclude', [])}


def world_row(w, how):
    return {
        'key': 'world/' + slug(w['id']),
        'name': w['name'],
        'playfield': w['playfield'],
        'kind': w['kind'],
        'region': D.region_name(w.get('region')) if w.get('region') else 'Other',
        'element': w.get('element'),
        'minifig': D.minifigs[w['minifigure']]['name'] if w.get('minifigure') in D.minifigs else None,
        'level_cap': (w.get('level') or {}).get('cap'),
        'reached_by': how,
    }


def _worlds(pocket_adventures):
    live = live_worlds()
    extra = {'world/' + s for s in D.manual['worlds'].get('live_extra', [])}
    out = []
    for wid in sorted(live, key=lambda i: D.worlds[i]['playfield'] if i in D.worlds else 0):
        w = D.worlds.get(wid)
        if not w or w['kind'] in ('hub', 'other', 'instance'):
            continue
        if bool(w.get('minifigure')) != pocket_adventures:
            continue
        out.append(world_row(w, 'manual.json' if wid in extra else 'portal'))
    return out


def adventure_zones():
    """Open worlds and adventures (non-pocket-adventure worlds) accessible in live."""
    return _worlds(False)


def pocket_adventures():
    """Pocket adventures (worlds that reward a minifigure) accessible in live."""
    return _worlds(True)


# ---- 11. Lost Creations ---------------------------------------------------------------------

def lost_creations():
    variants = D.manual['lost_creations']['variants']
    out = []
    for lc in sorted(D.lcs.values(), key=lambda r: r['name']):
        s = slug(lc['id'])
        out.append({
            'key': 'lc/' + s,
            'name': lc['name'],
            'mode': lc.get('mode'),
            'playfield': D.worlds.get(lc['world'], {}).get('playfield'),
            'variants': [{'key': f'lc/{s}/{re.sub(r"\W+", "-", v.lower())}', 'name': v} for v in variants.get(s, [])],
        })
    return out


# ---- 12. dialogues --------------------------------------------------------------------------

NOISE = re.compile(r'(vox|vocal|music|emote|fx\b|smoke|^talk|stinger|shake|laugh|teleport|^stop |^spawn |quest|'
                   r'cheer|wave|dance|shrug|nod|worr|scary|angry|excited|awkward|frustrated|incredulous|'
                   r'^back to normal|crowd|radio|^give |^with|^without|joins the player|^point$|ouch|record scratch|disgusted)', re.I)


def clean_text(s):
    s = re.sub(r'\{[^}]*\}', ' ', s)
    s = re.sub(r'\[([^\]]+)\]', lambda m: '' if NOISE.search(m.group(1).strip()) else m.group(1).strip() + ': ', s)
    return re.sub(r'\s+', ' ', s).strip()


def conversations():
    """The client's dialogue texts (TDB 50002) in conversations: a text the client answers into
    (it has a 50001 answer) continues the conversation before it."""
    t = D.texts['texts']
    answers = D.texts['answers']
    convs = []
    for i in sorted(map(int, t)):
        if str(i) in answers and convs and i - convs[-1][-1] <= 6:
            convs[-1].append(i)
        else:
            convs.append([i])
    return convs


def dialogues():
    """{world: conversation rows}; 'other' holds tests and texts with no spoken line."""
    t = D.texts['texts']
    cfg = D.manual['dialogues']
    world_of = {s: w for w, ss in cfg['speakers'].items() for s in ss}
    overrides = cfg.get('texts', {})
    triggered = {}
    for d in D.dialogues.values():
        for i in d.get('texts', []):
            triggered[i] = d
    groups = collections.defaultdict(list)
    for c in conversations():
        speakers = collections.Counter()
        for i in c:
            for m in re.findall(r'\[([^\]]+)\]', t[str(i)]):
                if not NOISE.search(m.strip()):
                    speakers[m.strip()] += 1
        lines = [clean_text(t[str(i)]) for i in c]
        votes = collections.Counter()
        for s, n in speakers.items():
            if s in world_of:
                votes[world_of[s]] += n
        if str(c[0]) in overrides:
            world = overrides[str(c[0])]
        elif not any(lines):
            world = 'other'
        elif votes:
            world = votes.most_common(1)[0][0]
        else:
            world = 'Unassigned'
        hits = [triggered[i] for i in c if i in triggered]
        groups[world].append({
            'key': f'dialogue/{c[0]}',
            'texts': c,
            'speakers': [s for s, _ in speakers.most_common()],
            'preview': next((l for l in lines if l), '')[:140],
            'triggered': bool(hits),
            'trigger': slug(hits[0]['id']) if hits else None,
        })
    return dict(groups)


def _starts():
    """dialogue id -> what starts it: story trigger areas (worlds/*/triggers.json) and cinematics'
    `then_dialogue` (tunables.json `story.cinematics`)."""
    out = collections.defaultdict(list)
    for wid, w in D.worlds.items():
        p = os.path.join(w['_dir'], 'triggers.json')
        if not os.path.exists(p):
            continue
        with open(p) as fh:
            for t in json.load(fh).get('triggers', []):
                for a in t.get('actions', []):
                    d = a.get('dialogue', {}).get('dialogue')
                    if d:
                        out[d].append(f'area {t.get("name") or slug(t.get("area"))}')
    with open(os.path.join(CONTENT, 'tunables.json')) as fh:
        tun = json.load(fh)
    for name, c in tun.get('story', {}).get('cinematics', {}).items():
        if c.get('then_dialogue'):
            out[c['then_dialogue']].append(f'after the cinematic {name}')
    return out


def rebuilt():
    """{world: rows}: the rebuilt story conversations (content/dialogues tag `reconstructed`, made by
    tools/content/story_dialogues.py from the client's texts), each to be checked against footage."""
    starts = _starts()
    by_first = {d['texts'][0]: d for d in D.dialogues.values() if d.get('texts')}
    groups = collections.defaultdict(list)
    for d in sorted(D.dialogues.values(), key=lambda d: (d.get('order', 0), d['id'])):
        tags = d.get('tags', [])
        if 'reconstructed' not in tags:
            continue
        place = list(starts.get(d['id'], []))
        if d.get('defeated'):
            place.insert(0, 'after defeating ' + ('its world\'s boss' if d['defeated'] == 'boss' else d['defeated']))
        if d.get('follows'):
            f = by_first.get(d['follows'])
            place.insert(0, f'after dialogue {d["follows"]}' + (f' ({slug(f["id"])})' if f else ''))
        conditions = []
        if d.get('requires'):
            conditions.append('needs tags ' + ', '.join(map(str, d['requires'])))
        if d.get('unless'):
            conditions.append('not with ' + ', '.join(map(str, d['unless'])))
        if d.get('goals'):
            conditions.append('goals ' + ', '.join(d['goals']))
        w = D.worlds.get(d.get('world'))
        speakers = collections.Counter()
        for i in d['texts']:
            for m in re.findall(r'\[([^\]]+)\]', D.texts['texts'].get(str(i), '')):
                if not NOISE.search(m.strip()):
                    speakers[m.strip()] += 1
        groups[D.world_name(d.get('world'))].append({
            'key': f'rebuilt/{slug(d["id"])}',
            'name': d.get('name', slug(d['id'])),
            'dialogue': slug(d['id']),
            'playfield': w.get('playfield') if w else None,
            'texts': d['texts'],
            'place': place or ['not started by anything (!dialog only)'],
            'conditions': conditions,
            'speakers': [x for x, _ in speakers.most_common()],
            'owner': d.get('npc') or '',
            'confidence': next((t for t in tags if t in ('likely', 'guess', 'certain')), d.get('source', '')),
            'pos': d.get('pos'),
        })
    return dict(groups)


LISTS = collections.OrderedDict([
    ('upgrades', upgrades),
    ('characters', characters),
    ('enemies', enemies),
    ('enemy-abilities', enemy_abilities),
    ('world-achievements', world_achievements),
    ('global-achievements', global_achievements),
    ('minifig-achievements', minifig_achievements),
    ('area-quests', area_quests),
    ('adventure-zones', adventure_zones),
    ('pocket-adventures', pocket_adventures),
    ('lost-creations', lost_creations),
    ('dialogues', dialogues),
    ('rebuilt', rebuilt),
])


def main(argv):
    if len(argv) < 2 or argv[1] not in LISTS:
        print(__doc__)
        print('lists: ' + ', '.join(LISTS))
        return 1
    data = LISTS[argv[1]]()
    if '--json' in argv:
        json.dump(data, sys.stdout, indent=1, ensure_ascii=False)
        print()
        return 0
    rows = data.items() if isinstance(data, dict) else [(None, data)]
    for group, items in rows:
        if group is not None:
            print(f'== {group} ({len(items)})')
        for r in items:
            print('  ' * r.get('depth', 0) + ' | '.join(str(v) for k, v in r.items() if k not in ('key', 'depth')))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
