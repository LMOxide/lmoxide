"""Playfield pathfinding data (RDB 1000051, GameData::RDBPathData_c -> CorePathfinding::
PlayfieldPathData::Read / PathData::LoadDataFromBlob in the Linux build).

  i32 version (4), i32 cell count, cells: { i32 id, vec3 min, vec3 max (an axis-aligned walkable
  box, ~20 m across; y spans the ground height inside it), i32 neighbour count, neighbours:
  { CellContactPoints (v>=4: u16 side, u16, vec3 contact; v<4: 4 x i16), i32 cell id,
  [v>=4] f32 cost } }, then the NavCellTree: u32 0xFACED000, u32 size, flat nodes.

  navmesh.py PLAYFIELD      summary of one playfield's cells
"""
import struct, sys

from rdb import body, index


def cells(pf, idx=None):
    """[{id, min, max, neighbours: [cell id]}] or None when the playfield has no path data."""
    try:
        b = body(1000051, pf, idx or index())
    except (KeyError, FileNotFoundError):
        return None
    ver, n = struct.unpack_from("<2i", b, 0)
    o, out = 8, []
    for _ in range(n):
        cid = struct.unpack_from("<i", b, o)[0]
        lo, hi = struct.unpack_from("<3f", b, o + 4), struct.unpack_from("<3f", b, o + 16)
        nadj = struct.unpack_from("<i", b, o + 28)[0]
        o += 32
        nb = []
        for _ in range(nadj):
            if ver >= 4:
                nb.append(struct.unpack_from("<i", b, o + 16)[0])
                o += 24
            else:
                nb.append(struct.unpack_from("<i", b, o + 8)[0])
                o += 12
        out.append(dict(id=cid, min=lo, max=hi, neighbours=nb))
    assert struct.unpack_from("<I", b, o)[0] == 0xFACED000, "navmesh parse out of step"
    return out


def contains(c, p, margin=0.0):
    return all(c["min"][i] - margin <= p[i] <= c["max"][i] + margin for i in (0, 2))


if __name__ == "__main__":
    cs = cells(int(sys.argv[1]))
    if cs is None:
        sys.exit("no path data")
    xs = [c["min"][0] for c in cs] + [c["max"][0] for c in cs]
    zs = [c["min"][2] for c in cs] + [c["max"][2] for c in cs]
    sizes = sorted(round(c["max"][0] - c["min"][0]) for c in cs)
    print(f"{len(cs)} cells, x {min(xs):.0f}..{max(xs):.0f}, z {min(zs):.0f}..{max(zs):.0f}, "
          f"cell width median {sizes[len(sizes) // 2]} (min {sizes[0]}, max {sizes[-1]})")
