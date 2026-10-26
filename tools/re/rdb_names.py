"""RDB resource name index (RDB type 1020004): resource name <-> resource id.

  rdb_names.py TYPE           every "id name" of one resource type (reconstructed, see below)
  rdb_names.py TYPE NAME      the id the client resolves NAME to (exact; nothing if unknown)

The client's class is `Patricia` (symbols in the Linux build; Windows LMO.exe: Load 0x4ae4d0,
Lookup 0x4ae370, encode 0x4ae1d0, alphabet 0x4ae120; per-type map built by 0xc728f0, typed
lookup 0xc726e0). RDB 1020004 has one resource per indexed type, the resource id being that type
(1010001, 1010207 particles, 1020002 sounds, 1020005 music, 1050002, ...; five are empty).

Body (little endian):
  u32 nnodes, u32 nstrings, u32 stringlen
  nnodes x 20-byte node: u64 value (resource id; 0 on inner nodes), u32 base, u32 depth, u32 str
  u8 pool[stringlen]       NUL-terminated fragments, in the 44-symbol code below
  u32 strings[nstrings]    start offset of every fragment the builder appended (not used by Lookup)

Code: 0 end, 1-26 a-z (either case), 27 '\\' and '/', 28 '\\n', 29 '\\r', 30 '-', 31 '_',
32 every other byte (':' ' ' ...), 33 '.', 34-43 '0'-'9'. Names are compared in this code, so
lookups ignore case and treat '/' as '\\'.

It is a double array without check field (a compressed radix trie). Lookup(name): e = code(name),
n = len(e); base = 0, pos = 0, built = e[0]; loop: node = nodes[base + c] (c = e[pos], fail if
out of range); if c != 0, append pool[node.str] to built; fail unless pos <= node.depth <= n;
if node.depth == n the result is node.value if built == e; else pos = node.depth + 1,
built = built[:pos] + e[pos], c = e[pos] (0 past the end), base = node.base. So an inner node
covers name[0..depth] (its fragment is the edge label after the branch character), a leaf has
depth = len(name), and a name that is a prefix of another is the c = 0 child (whose fragment is
leftover garbage from the builder's key buffer and is not compared). The branch characters are
not stored and nothing is verified but the final compare, so a lookup also accepts some wrong
names (whatever a wrong branch character spells); names() lists the real ones.

Builder (Linux LMO Patricia::CompressNodes/AddString): depth-first, children in code order.
For a node, the bases tried start at the first free slot and advance by d, where d = 1 if slot
B+45 is free, else the smallest d >= 2 with slot B+44+d free; the first B whose child slots
B+c are all free becomes the node's base and those slots are marked used. Then for each child in
code order: AddString(fragment) (reuse the first earlier fragment ending with it, else append it
to the pool and the strings table), and recurse. Every inner node has >= 2 children.

names() inverts this: since a node does not store its parent or branch character, the parent of
each slot is found by replaying the builder: candidate parents are those whose depth fits
(len(parent prefix) + 1 + len(fragment) = depth + 1 for inner nodes, = depth for leaves), and a
choice is kept only if every base comes out as the builder would place it and every fragment is
appended in the pool's order. A character n-gram model trained on unambiguous names orders the
choices. The replay completes for 1010002/3/28/29/200/201/207, 1020005 and 1050002; for
1010002/3/200/201/207 and 1020005 the recovered names rebuild the index byte for byte (the
others hold c = 0 children with garbage fragments, which depend on insertion order). A few
choices the index cannot tell apart (two numbered sets sharing slots: 1020005 reads
music_champion_combat_6 where the effects say music_miniboss_combat_3) are decided by the model.
For 1010001/4/30 and 1020002 the search gives up partway (stderr says where); the rest is
finished greedily and those names are partly guesses (1020002: 2284 of the 2403 sound paths in
data/effect_catalog.json come out exactly). lookup() is exact for every type.
"""
import itertools, math, os, struct, sys, threading
from collections import Counter, defaultdict
from functools import lru_cache

import rdb

INDEX = 1020004
ALPHA = "\0abcdefghijklmnopqrstuvwxyz\\\n\r-_ .0123456789"
M = len(ALPHA)  # 44, Patricia::s_MaxLegal
EMPTY = (0, 0, 0, 0)
CODE = [32] * 256
for _i, _ch in enumerate(ALPHA):
    if _i:
        CODE[ord(_ch)] = _i
for _ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ":
    CODE[ord(_ch)] = CODE[ord(_ch.lower())]
CODE[ord("/")] = 27
CODE[0] = 0


def encode(name):
    return bytes(CODE[b] for b in name.encode("latin1"))


def decode(e):
    return "".join(ALPHA[c] for c in e)


@lru_cache(None)
def load(rtype, idx=None):
    """(nodes, pool, string offsets) of the name index of one resource type."""
    b = rdb.body(INDEX, rtype, idx or _idx())
    n, f, p = struct.unpack_from("<3I", b)
    nodes = [struct.unpack_from("<QIII", b, 12 + 20 * i) for i in range(n)]
    pool = b[12 + 20 * n : 12 + 20 * n + p]
    offs = struct.unpack_from(f"<{f}I", b, 12 + 20 * n + p)
    return nodes, pool, offs


@lru_cache(None)
def _idx():
    return rdb.index()


def frag(pool, o):
    return pool[o : pool.index(0, o)]


def lookup(rtype, name, idx=None):
    """Resource id the client resolves `name` to (Patricia::Lookup), or None."""
    nodes, pool, _ = load(rtype, idx)
    e = encode(name)
    n = len(e)
    if not n or not nodes:
        return None
    base, pos, c = 0, 0, e[0]
    built = bytearray([c])
    while base + c < len(nodes):
        node = nodes[base + c]
        if c:
            built += frag(pool, node[3])
        d = node[2]
        if d < pos or d > n:
            return None
        if d == n:
            return (node[0] or None) if bytes(built) == e else None
        pos = d + 1
        c = (e + b"\0")[pos]
        del built[pos:]
        built.append(c)
        base = node[1]
    return None


# -- reconstruction --------------------------------------------------------------------------

def candidates(nodes, pool):
    """slot -> [(parent, c)] allowed by depths; parent -1 is the root (base 0, prefix 0)."""
    zero = pool[:1] == b"\0"  # an all-zero slot can then be a real inner node (depth 0, base 0)
    flen = [len(frag(pool, x[3])) for x in nodes]
    inner = lambda x: not x[0] and (x != EMPTY or zero)
    parents = [(-1, 0, 0)] + [(q, x[1], x[2] + 1) for q, x in enumerate(nodes) if inner(x)]
    par = defaultdict(list)
    for q, base, p in parents:
        for c in range(M):
            i = base + c
            if i >= len(nodes):
                break
            rid, _, d, _ = nodes[i]
            if c == 0:
                if rid and d == p:
                    par[i].append((q, c))
            elif d >= p:
                ln = p + 1 + flen[i]
                if (rid and d == ln) or (inner(nodes[i]) and d + 1 == ln):
                    par[i].append((q, c))
    return par


class LM:
    """Interpolated character n-gram model over coded names (orders the replay's choices)."""

    def __init__(self, texts, order=6):
        self.order, self.c = order, defaultdict(Counter)
        for t in texts:
            t += b"\0"
            for i in range(len(t)):
                for k in range(min(order, i + 1)):
                    self.c[t[i - k : i]][t[i]] += 1

    def score(self, ctx, seq):
        out = 0.0
        for ch in seq:
            p = 1 / 45
            for k in range(min(self.order, len(ctx) + 1)):
                cnt = self.c.get(ctx[len(ctx) - k :] if k else b"")
                if not cnt:
                    break
                tot = sum(cnt.values())
                lam = tot / (tot + len(cnt))
                p = lam * cnt[ch] / tot + (1 - lam) * p
            out += math.log(p)
            ctx += bytes([ch])
        return out


def _prefixes(nodes, pool, par, pick):
    """Inner-node prefixes and leaf names, top-down, choosing each slot's parent with pick()."""
    pref, out = {-1: b""}, {}
    for i in sorted(par, key=lambda i: (nodes[i][2], bool(nodes[i][0]))):
        opts = [(q, c) for q, c in par[i] if q in pref]
        if not opts:
            continue
        q, c = pick(i, opts, pref)
        x = nodes[i]
        t = pref[q] if c == 0 else pref[q] + bytes([c]) + frag(pool, x[3])
        if x[0]:
            out[x[0]] = t
        else:
            pref[i] = t[: x[2] + 1]
    return pref, out


def unambiguous(rtype, idx=None):
    nodes, pool, _ = load(rtype, idx)
    par = {i: v for i, v in candidates(nodes, pool).items() if len(v) == 1}
    return list(_prefixes(nodes, pool, par, lambda i, o, p: o[0])[1].values())


@lru_cache(None)
def model(idx=None):
    """n-gram model trained on the names every index spells unambiguously."""
    types = sorted(i for t, i in (idx or _idx()) if t == INDEX)
    return LM([n for t in types for n in unambiguous(t, idx)])


class Replay:
    """Search for the parent of every slot by replaying Patricia::CompressNodes (see module doc)."""

    def __init__(self, rtype, lm, idx=None, budget=100000):
        self.nodes, self.pool, offs = load(rtype, idx)
        nodes, pool = self.nodes, self.pool
        self.N, self.lm, self.budget = len(nodes), lm, budget
        strs = [frag(pool, o) for o in offs]
        self.nstr = len(strs)
        zero = pool[:1] == b"\0"
        par = candidates(nodes, pool)
        self.par = par
        self.kidc = defaultdict(list)
        for i, v in par.items():
            for q, c in v:
                self.kidc[q].append((c, i))
        for v in self.kidc.values():
            v.sort()
        # AddString timing: first pool fragment each slot's fragment is a suffix of
        self.j, exact = {}, {}
        for i, x in enumerate(nodes):
            if x != EMPTY or zero:
                f = frag(pool, x[3])
                self.j[i] = next(j for j, s in enumerate(strs) if s.endswith(f))
                exact[i] = f == strs[self.j[i]]
        nex = Counter(self.j[i] for i in self.j if exact[i])
        self.uniq = {i for i in self.j if exact[i] and nex[self.j[i]] == 1}  # sole appender
        self.samep = {i: len({nodes[q][2] + 1 if q >= 0 else 0 for q, _ in v}) == 1
                      for i, v in par.items()}
        self._windows(exact)
        self.used = bytearray(self.N + 4 * M + 10)
        self.done, self.kids, self.pref = set(), {}, {-1: b""}
        self.steps, self.soft, self.broken, self.result = 0, False, 0, None
        self.est = _prefixes(nodes, pool, par, self._lmpick)[0]

    def _windows(self, exact):
        """Bounds on the string count k at which each slot can be visited."""
        nodes, par = self.nodes, self.par
        self.tmin = {i: self.j[i] + (0 if exact[i] else 1) for i in self.j}
        self.tmax = {i: self.j[i] if i in self.uniq else self.nstr for i in self.j}
        sole = defaultdict(list)
        for u, v in par.items():
            if len(v) == 1 and v[0][0] >= 0:
                sole[v[0][0]].append(u)
        inner = sorted((i for i in self.j if not nodes[i][0]), key=lambda i: -nodes[i][2])
        for _ in range(3):
            for i in inner:
                for u in sole.get(i, ()):
                    self.tmax[i] = min(self.tmax[i], self.tmax.get(u, self.nstr))
                ks = [self.tmin[u] for _, u in self.kidc.get(i, ()) if u in self.tmin]
                if ks:
                    self.tmin[i] = max(self.tmin[i], min(ks) - exact[i])
            for i in reversed(inner):
                for u in sole.get(i, ()):
                    if u in self.tmin:
                        self.tmin[u] = max(self.tmin[u], self.tmin[i])

    def _junction(self, q, c, i, pref):
        p = pref.get(q, b"")
        seq = (bytes([c]) + frag(self.pool, self.nodes[i][3]))[:8] if c else b"\0"
        return self.lm.score(p, seq)

    def _lmpick(self, i, opts, pref):
        return max(opts, key=lambda o: self._junction(o[0], o[1], i, pref))

    def run(self):
        threading.stack_size(1024 * 1024 * 1024)
        old = sys.getrecursionlimit()
        sys.setrecursionlimit(10 ** 7)
        th = threading.Thread(target=lambda: self._compress(-1, [], 0))
        th.start()
        th.join()
        sys.setrecursionlimit(old)
        return self.result

    # entries: pending children (top of the stack is visited next); k = fragments appended so far
    def _visit(self, stack, k):
        self.steps += 1
        if not stack:
            ok = k == self.nstr and all(self.used[i] or self.nodes[i] == EMPTY for i in range(self.N))
            if ok or self.soft:
                self.broken += not ok
                self.result = dict(self.kids)
                return True
            return False
        i, rest = stack[-1], stack[:-1]
        j = self.j[i]
        k2 = k + (j == k)
        bad = j > k or (i in self.uniq and j != k) or not self.tmin[i] <= k <= self.tmax[i]
        bad = bad or (k2 != k and any(r in self.uniq and self.j[r] < k2 for r in rest))
        if bad:
            if not self.soft:
                return False
            self.broken += 1
            k2 = max(k, j + 1)
        if self.nodes[i][0]:
            return self._visit(rest, k2)
        return self._compress(i, rest, k2)

    def _bases(self, base):
        """Bases the builder tries before reaching `base` (None if it jumps over it)."""
        used, b, out = self.used, 0, []
        while used[b]:
            b += 1
        while b < base:
            out.append(b)
            d = 1
            if used[b + M + 1]:
                d = 2
                while used[b + M + d]:
                    d += 1
            b += d
        return out if b == base else None

    def _compress(self, q, rest, k):
        nodes, used = self.nodes, self.used
        tried = self._bases(0 if q < 0 else nodes[q][1])
        if tried is None and not self.soft:
            return False
        tried = tried or []
        free = [(c, i) for c, i in self.kidc.get(q, []) if not used[i]]
        self.done.add(q)
        forced, opt = [], []
        for c, i in free:
            others = [p for p, _ in self.par[i] if p != q and p not in self.done]
            if nodes[i] == EMPTY:
                opt.append((c, i))
            elif not others:
                forced.append((c, i))
            elif i in self.uniq and any(p in self.uniq and self.j[p] < self.j[i] for p in others):
                continue  # another candidate parent is visited before i
            elif i in self.uniq and all(self.j[p] > self.j[i] for p in others):
                forced.append((c, i))  # every other candidate is visited after i
            else:
                opt.append((c, i))
        cuts = self._cuts(q, rest, forced + opt)
        start = self.steps
        for kid in [] if self.soft else self._choices(q, forced, opt, cuts, tried, rest, k):
            if self.steps - start > self.budget and not self.soft:
                break
            if self._take(q, kid, rest, k):
                return True
        if self.soft or self.steps - start > self.budget:
            if not self.soft:
                self.gaveup = (q, len(self.done), k)
            self.soft = True  # give up searching: finish greedily, counting broken rules
            kid = next(self._choices(q, forced, opt, cuts, tried, rest, k, strict=False), None)
            return self._take(q, kid or sorted(forced or opt[:1]), rest, k)
        self.done.discard(q)
        return False

    def _cuts(self, q, rest, cands):
        """Possible fragment counts E at the end of q's subtree (from the next pending entry)."""
        if q < 0:
            return [self.nstr]
        lo, hi = None, self.nstr
        for n, r in enumerate(reversed(rest)):
            if n == 0:
                lo = self.j[r]
            if r in self.uniq:
                hi = self.j[r]
                break
        lo = hi if lo is None else lo
        if rest:  # E is the k at which the next pending entry is visited
            lo, hi = max(lo, self.tmin[rest[-1]]), min(hi, self.tmax[rest[-1]])
        us = [self.j[i] for _, i in cands if i in self.uniq and self.samep.get(i)]
        return sorted({e for e in [hi] + us if lo <= e <= hi})

    def _choices(self, q, forced, opt, cuts, tried, rest, k, strict=True):
        """Child sets for q: slots whose visit time lies inside q's subtree, best model score first."""
        seen, fallback = set(), []
        for e in cuts or [self.nstr]:
            must, may = list(forced), []
            for c, i in forced:
                if strict and self.samep.get(i) and self.j[i] >= e or self.tmin[i] > e or self.tmax[i] < k:
                    must = None
                    break
            if must is None:
                continue
            for c, i in opt:
                if (self.samep.get(i) and self.j[i] >= e) or self.tmin[i] > e or self.tmax[i] < k:
                    continue
                (must if i in self.uniq and self.samep.get(i) else may).append((c, i))
            combs = self._order(q, may)
            for comb in combs if strict else itertools.islice(combs, 64):
                kid = tuple(sorted(must + list(comb)))
                if kid in seen or len(kid) < (1 if q < 0 else 2):
                    continue
                seen.add(kid)
                ok = self._legal(q, kid, tried, rest, k)
                if ok or not strict:
                    if ok or strict:
                        yield list(kid)
                    else:
                        fallback.append(list(kid))
        if not strict:
            self.broken += 1
            yield from fallback

    def _legal(self, q, kid, tried, rest, k):
        used = self.used
        if any(not any(used[b + c] for c, _ in kid) for b in tried):
            return False  # the builder would have placed q's children at an earlier base
        if self.j[kid[0][1]] > k:
            return False
        last = k  # sole appenders must come in pool order
        for _, i in kid:
            if self.j[i] > k and i in self.uniq:
                if self.j[i] < last:
                    return False
                last = self.j[i]
        for r in reversed(rest):
            if self.j[r] > k and r in self.uniq:
                if self.j[r] <= last:
                    return False
                break
        taken = {i for _, i in kid}  # unvisited inner candidates must keep >= 2 possible children
        for _, i in kid:
            for p, _ in self.par.get(i, ()):
                if p == q or p < 0 or p in self.done or (self.nodes[p] == EMPTY and not used[p]):
                    continue
                if sum(1 for _, u in self.kidc.get(p, ()) if not used[u] and u not in taken) < 2:
                    return False
        return True

    def _order(self, q, may):
        if len(may) > 14:
            for n in range(len(may) + 1):
                yield from itertools.combinations(may, n)
            return
        pref = []
        for c, i in may:
            mine = self._junction(q, c, i, self._pf())
            other = max((self._junction(p, cc, i, self._pf()) for p, cc in self.par[i]
                         if p != q and p not in self.done), default=-1e9)
            pref.append(mine - other)
        def cost(mask):
            return sum(-v if mask >> b & 1 else v for b, v in enumerate(pref)
                       if (v < 0) == bool(mask >> b & 1))
        for mask in sorted(range(1 << len(may)), key=cost):
            yield tuple(m for b, m in enumerate(may) if mask >> b & 1)

    def _pf(self):
        return _Pref(self.pref, self.est)

    def _take(self, q, kid, rest, k):
        used = self.used
        for _, i in kid:
            used[i] = 1
        self.kids[q] = kid
        p = self.pref.get(q, self.est.get(q, b""))
        for c, i in kid:
            x = self.nodes[i]
            if not x[0]:
                t = p if c == 0 else p + bytes([c]) + frag(self.pool, x[3])
                self.pref[i] = t[: x[2] + 1]
        dead = []
        if q < 0:  # all-zero slots the root did not take are empty
            dead = [i for i in range(min(M, self.N)) if self.nodes[i] == EMPTY and not used[i]]
            self.done.update(dead)
        if self._visit(rest + [i for _, i in reversed(kid)], k):
            return True
        for _, i in kid:
            used[i] = 0
        del self.kids[q]
        self.done.difference_update(dead)
        return False


class _Pref:
    """Known prefixes, falling back to the model-only estimate for unvisited nodes."""

    def __init__(self, known, est):
        self.known, self.est = known, est

    def get(self, k, default=None):
        return self.known.get(k, self.est.get(k, default))


def tree_names(nodes, pool, kids):
    out, stack = {}, [(-1, b"")]
    while stack:
        q, p = stack.pop()
        for c, i in kids.get(q, []):
            x = nodes[i]
            t = p if c == 0 else p + bytes([c]) + frag(pool, x[3])
            if x[0]:
                out[x[0]] = t
            else:
                stack.append((i, t[: x[2] + 1]))
    return out


@lru_cache(None)
def _names(rtype, idx=None):
    nodes, pool, _ = load(rtype, idx)
    if not nodes:
        return {}
    r = Replay(rtype, model(idx), idx)
    got = tree_names(nodes, pool, r.run() or {})
    if r.broken:
        q, n, k = r.gaveup
        print(f"rdb_names: {rtype}: replay gave up at node {q} ({n} inner nodes, {k} fragments in) "
              f"and broke {r.broken} builder rules after it; those names are guesses",
              file=sys.stderr)
        # leaves the greedy finish lost: the model-only reading
        lm = _prefixes(nodes, pool, r.par, r._lmpick)[1]
        got = {**lm, **got}
    return {rid: decode(e) for rid, e in got.items()}


def names(rtype, idx=None):
    """id -> name (lowercase, '\\' separators, as the client's index spells it)."""
    return dict(_names(rtype, idx))


def main():
    rtype = int(sys.argv[1])
    if len(sys.argv) > 2:
        rid = lookup(rtype, sys.argv[2])
        if rid is not None:
            print(rid)
        return
    for rid, name in sorted(names(rtype).items(), key=lambda x: x[1]):
        print(rid, name)


if __name__ == "__main__":
    main()
