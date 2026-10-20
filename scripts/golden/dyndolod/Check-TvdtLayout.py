#!/usr/bin/env python3
"""Check-TvdtLayout.py: derive and verify the CELL TVDT bit layout from a golden Occlusion.esp.

Run with an isolated interpreter (no site-packages, no cwd on sys.path):

    python3 -I scripts\\golden\\dyndolod\\Check-TvdtLayout.py ^
        D:\\Modlists\\LODForge-golden\\dyndolod-20261017-medium-tamriel\\output\\Occlusion.esp ^
        --origin -96,-96 --stride 256 --radius 100 ^
        --other-plugin ...\\output\\DynDOLOD.esm --other-plugin ...\\output\\DynDOLOD.esp ^
        --json bench\\tvdt-layout.json

Standard library only. It reads the plugin with its own minimal TES5 record parser (24-byte
record headers, zlib-compressed records, XXXX size overrides) and then:

1. reports the plugin header (ESL flag, HEDR, author, description, masters, INCC), the
   worldspaces in file order, the subrecords copied for WRLD and CELL, and the TVDT / MHDT sizes;
2. STRUCTURE: among candidate bit layouts (quadtree traversal order x bit numbering x stream
   direction), finds the ones for which every parent bit equals the OR of its four children in
   every cell. The candidates only use the public facts: TVDT is a z-order (Morton) bitmap with
   one bit per block at each LOD level (4x4 cells up to the whole grid), the grid is `stride`
   cells square with its south-west corner at the lodsettings origin;
3. GEOMETRY: for the surviving structure(s), among the 8 symmetries of the square (which corner
   leaf 0 maps to, which axis is the high Morton bit) finds the mapping for which the observer's
   own 4x4 block is set in every cell and every set block lies within the occlusion radius;
   shows that shifting the origin by whole blocks breaks that;
4. prints the measured counts (meaningful bits, trailing zero bits, root bit, set blocks per
   cell, max distance of a set block, MHDT offsets) so a spec can cite them.

Exit code 1 when no candidate layout survives, or when any check in section 2/3 fails for the
survivor. Nothing here depends on how any generator computes visibility: it only reads output.
"""

import argparse
import json
import math
import struct
import sys
import zlib
from collections import Counter, OrderedDict

REC_HEADER = 24
COMPRESSED = 0x00040000
ESL = 0x00000200
MASTER = 0x00000001

# xEdit's CELL definition order (wbDefinitionsTES5.pas, record CELL), the order a copied CELL
# keeps its subrecords in.
CELL_ORDER = ["EDID", "FULL", "DATA", "XCLC", "XCLL", "TVDT", "MHDT", "LTMP", "LNAM", "XCLW",
              "XNAM", "XCLR", "XLCN", "XWCN", "XWCS", "XWCU", "XCWT", "XOWN", "XRNK", "XILL",
              "XWEM", "XCCM", "XCAS", "XEZN", "XCMO", "XCIM"]


# ----------------------------------------------------------------------------- plugin reading

def subrecords(data):
    """Yield (sig, bytes) for the subrecords of a record's (decompressed) data."""
    o, n, override = 0, len(data), None
    while o + 6 <= n:
        sig = data[o:o + 4].decode("latin-1")
        size = struct.unpack_from("<H", data, o + 4)[0]
        o += 6
        if sig == "XXXX":
            override = struct.unpack_from("<I", data, o)[0]
            o += size
            continue
        if override is not None:
            size, override = override, None
        yield sig, data[o:o + size]
        o += size


def record_data(buf, o, size, flags):
    raw = buf[o + REC_HEADER:o + REC_HEADER + size]
    if flags & COMPRESSED:
        return zlib.decompress(raw[4:])
    return raw


def walk(buf, o, end, world, out):
    """Walk records and groups in buf[o:end]; `world` is the enclosing WRLD (EDID) or None."""
    while o + REC_HEADER <= end:
        sig = buf[o:o + 4].decode("latin-1")
        if sig == "GRUP":
            size, label, gtype = struct.unpack_from("<I4si", buf, o + 4)
            walk(buf, o + REC_HEADER, o + size, world, out)
            o += size
            continue
        size, flags, form_id = struct.unpack_from("<III", buf, o + 4)
        data = record_data(buf, o, size, flags)
        subs = list(subrecords(data))
        if sig == "WRLD":
            w = OrderedDict(form_id=form_id, flags=flags, subs=[s for s, _ in subs], cells=[])
            for s, d in subs:
                if s == "EDID":
                    w["edid"] = d.rstrip(b"\0").decode("latin-1")
                elif s == "WNAM":
                    w["parent"] = struct.unpack("<I", d)[0]
                elif s == "PNAM":
                    w["pnam"] = struct.unpack("<H", d)[0] if len(d) >= 2 else d[0]
                elif s == "DNAM" and len(d) >= 8:
                    w["dnam"] = struct.unpack("<ff", d[:8])
                elif s == "NAM4" and len(d) >= 4:
                    w["nam4"] = struct.unpack("<f", d[:4])[0]
                elif s == "DATA" and len(d) >= 1:
                    w["data_flags"] = d[0]
                elif s == "OFST":
                    w["ofst_bytes"] = len(d)
            out["worlds"].append(w)
            world = w
        elif sig == "CELL" and world is not None:
            c = dict(form_id=form_id, flags=flags, subs=[s for s, _ in subs])
            for s, d in subs:
                if s == "XCLC" and len(d) >= 8:
                    c["grid"] = struct.unpack("<ii", d[:8])
                elif s == "TVDT":
                    c["tvdt"] = d
                elif s == "MHDT":
                    c["mhdt"] = d
            world["cells"].append(c)
        o += REC_HEADER + size


def read_plugin(path):
    buf = open(path, "rb").read()
    out = dict(path=path, worlds=[], header={})
    sig = buf[:4]
    if sig != b"TES4":
        raise SystemExit(f"{path}: not a TES4 plugin")
    size, flags, _ = struct.unpack_from("<III", buf, 4)
    h = out["header"]
    h["flags"] = flags
    h["esl"] = bool(flags & ESL)
    h["master"] = bool(flags & MASTER)
    h["masters"] = []
    for s, d in subrecords(record_data(buf, 0, size, flags)):
        if s == "HEDR":
            h["version"], h["records"], h["next_object_id"] = struct.unpack("<fII", d[:12])
        elif s == "CNAM":
            h["author"] = d.rstrip(b"\0").decode("latin-1")
        elif s == "SNAM":
            h["description"] = d.rstrip(b"\0").decode("latin-1")
        elif s == "MAST":
            h["masters"].append(d.rstrip(b"\0").decode("latin-1"))
        elif s == "INCC":
            h["incc"] = struct.unpack("<I", d)[0]
    walk(buf, REC_HEADER + size, len(buf), None, out)
    out["bytes"] = len(buf)
    return out


# ----------------------------------------------------------------------------- quadtree layouts

def morton_decode(i):
    """(a, b) from an interleaved index: a from the odd bits, b from the even bits."""
    a = b = 0
    bit = 0
    while i:
        b |= (i & 1) << bit
        a |= ((i >> 1) & 1) << bit
        i >>= 2
        bit += 1
    return a, b


def level_sizes(nb):
    """Blocks per side at each level, leaves first: 64, 32, ..., 1."""
    sizes = []
    while nb >= 1:
        sizes.append(nb)
        nb //= 2
    return sizes


def stream_orders(nb):
    """Candidate assignments of stream index -> (level, morton index within the level).

    Each returns a list `nodes` where nodes[k] = (level, m) for stream index k, with level 0 the
    4x4-cell leaves and m the Morton index of the node within its level. Siblings are m//4 at the
    level above, which is all the parent/child test needs.
    """
    sizes = level_sizes(nb)
    levels = len(sizes)

    def post(level, m, acc):
        if level > 0:
            for c in range(4):
                post(level - 1, m * 4 + c, acc)
        acc.append((level, m))

    def pre(level, m, acc):
        acc.append((level, m))
        if level > 0:
            for c in range(4):
                pre(level - 1, m * 4 + c, acc)

    out = {}
    acc = []
    post(levels - 1, 0, acc)
    out["post-order (children, then parent)"] = acc
    acc = []
    pre(levels - 1, 0, acc)
    out["pre-order (parent, then children)"] = acc
    out["level by level, leaves first"] = [(l, m) for l in range(levels) for m in range(sizes[l] ** 2)]
    out["level by level, root first"] = [(l, m) for l in reversed(range(levels)) for m in range(sizes[l] ** 2)]
    return out


def bit_positions(total, numbering, direction, lead=0):
    """Bit position (byte*8 + bit) of stream index k for a numbering/direction candidate; `lead`
    bits before the stream are left out of it (a published decoder describes the field as 6 lead
    bits, a post-order tree, then 5 zero bits, so that description is tested as a candidate)."""
    pos = []
    for k in range(total):
        p = lead + (k if direction == "forward" else total - 1 - k)
        if numbering == "msb-first":
            p = (p & ~7) | (7 - (p & 7))
        pos.append(p)
    return pos


def bits_lsb(tvdt):
    """'0'/'1' string indexed by bit position p -> byte p>>3, bit p&7."""
    v = int.from_bytes(tvdt, "little")
    return format(v, f"0{len(tvdt) * 8}b")[::-1]


# ----------------------------------------------------------------------------- checks

def structure_candidates(cells, nb, total, sample):
    """Return {name: survivors} of the parent == OR(children) test on a sample, then on all cells."""
    orders = stream_orders(nb)
    sizes = level_sizes(nb)
    results = OrderedDict()
    for oname, nodes in orders.items():
        # stream index of each (level, m)
        index = {node: k for k, node in enumerate(nodes)}
        # (parent stream index, [child stream indices]) for every internal node
        pairs = []
        for l in range(1, len(sizes)):
            for m in range(sizes[l] ** 2):
                pairs.append((index[(l, m)], [index[(l - 1, m * 4 + c)] for c in range(4)]))
        for numbering, direction, lead in (("lsb-first", "forward", 0), ("lsb-first", "backward", 0),
                                           ("msb-first", "forward", 0), ("msb-first", "backward", 0),
                                           ("lsb-first", "forward", 6), ("lsb-first", "backward", 6)):
            if True:
                pos = bit_positions(total, numbering, direction, lead)
                name = f"{oname}; {numbering}; {direction}" + (f"; {lead} lead bits" if lead else "")
                ok_nodes = bad_nodes = 0
                bad_cells = 0
                for c in cells[:sample]:
                    s = bits_lsb(c["tvdt"])
                    bad = 0
                    for p, ch in pairs:
                        parent = s[pos[p]] == "1"
                        any_child = any(s[pos[k]] == "1" for k in ch)
                        if parent != any_child:
                            bad += 1
                    ok_nodes += len(pairs) - bad
                    bad_nodes += bad
                    bad_cells += bad > 0
                results[name] = dict(sample_cells=min(sample, len(cells)), bad_cells=bad_cells,
                                     bad_nodes=bad_nodes, ok_nodes=ok_nodes,
                                     nodes=nodes, pos=pos, pairs=pairs)
    return results


def full_structure_check(cells, cand):
    pairs, pos = cand["pairs"], cand["pos"]
    bad_cells = bad_nodes = 0
    for c in cells:
        s = bits_lsb(c["tvdt"])
        bad = 0
        for p, ch in pairs:
            if (s[pos[p]] == "1") != any(s[pos[k]] == "1" for k in ch):
                bad += 1
        bad_nodes += bad
        bad_cells += bad > 0
    return dict(cells=len(cells), bad_cells=bad_cells, bad_nodes=bad_nodes,
                internal_nodes_per_cell=len(pairs))


def leaf_streams(cells, cand, nb):
    """Per cell: the set of leaf Morton indices whose bit is set, plus root/trailing facts."""
    nodes, pos = cand["nodes"], cand["pos"]
    total = len(nodes)
    leaf_pos = [(m, pos[k]) for k, (l, m) in enumerate(nodes) if l == 0]
    root_k = next(k for k, (l, m) in enumerate(nodes) if l == len(level_sizes(nb)) - 1)
    used = set(pos)
    spare = [p for p in range(684 * 8) if p not in used]
    out = []
    for c in cells:
        s = bits_lsb(c["tvdt"])
        out.append(dict(leaves={m for m, p in leaf_pos if s[p] == "1"},
                        root=s[pos[root_k]] == "1", stray_bits=sum(s[p] == "1" for p in spare)))
    return out


def geometry(cells, leaves, nb, origin, stride, radius):
    """Try the 8 square symmetries for leaf index -> block; keep the ones that put the
    observer's own block in the set in every cell and every set block within the radius."""
    ox, oy = origin
    results = OrderedDict()
    for high_axis in ("x", "y"):
        for corner in ("north-east", "north-west", "south-east", "south-west"):
            def block_of(m):
                a, b = morton_decode(m)  # a: odd bits, b: even bits
                bx, by = (a, b) if high_axis == "x" else (b, a)
                if "east" in corner:
                    bx = nb - 1 - bx
                if "north" in corner:
                    by = nb - 1 - by
                return bx, by
            table = [block_of(m) for m in range(nb * nb)]
            index_of = {blk: m for m, blk in enumerate(table)}
            # Cheap test first: is the observer's own block set in every cell?
            own_set = 0
            for c, lv in zip(cells, leaves):
                gx, gy = c["grid"][0] - ox, c["grid"][1] - oy
                own_set += index_of.get((gx // 4, gy // 4)) in lv["leaves"]
            name = f"leaf 0 at the {corner} corner, {high_axis} in the high Morton bit"
            results[name] = dict(own_block_set=own_set, cells=len(cells), table=table)
    # Distances only for the mappings that passed (or all, when none did).
    passed = [n for n, r in results.items() if r["own_block_set"] == r["cells"]] or list(results)
    for name in passed:
        table = results[name]["table"]
        beyond = 0
        max_near = 0.0
        max_far = 0.0
        for c, lv in zip(cells, leaves):
            gx, gy = c["grid"][0] - ox, c["grid"][1] - oy
            for m in lv["leaves"]:
                bx, by = table[m]
                # distance from the observer cell to the nearest / farthest cell of the block
                dx = max(bx * 4 - gx, gx - (bx * 4 + 3), 0)
                dy = max(by * 4 - gy, gy - (by * 4 + 3), 0)
                near = math.hypot(dx, dy)
                fx = max(abs(bx * 4 - gx), abs(bx * 4 + 3 - gx))
                fy = max(abs(by * 4 - gy), abs(by * 4 + 3 - gy))
                far = math.hypot(fx, fy)
                max_near = max(max_near, near)
                max_far = max(max_far, far)
                if near > radius:
                    beyond += 1
        results[name].update(blocks_beyond_radius=beyond, max_nearest_cell_distance=round(max_near, 2),
                             max_farthest_cell_distance=round(max_far, 2))
    for name, r in results.items():
        r.setdefault("blocks_beyond_radius", None)
        r.setdefault("max_nearest_cell_distance", None)
        r.setdefault("max_farthest_cell_distance", None)
    return results


def origin_shift_table(cells, leaves, table, nb, origin, radius):
    """Own-block-set count and max Chebyshev block distance when the origin is shifted by whole
    blocks: the lodsettings origin should be the only one with every own block set and no block
    farther than ceil(radius/4) + 1 blocks."""
    ox, oy = origin
    rows = []
    boxes = []
    for c, lv in zip(cells, leaves):
        bxs = [table[m][0] for m in lv["leaves"]]
        bys = [table[m][1] for m in lv["leaves"]]
        boxes.append((min(bxs), max(bxs), min(bys), max(bys), set(table[m] for m in lv["leaves"])))
    for sx in (-8, -4, 0, 4, 8):
        for sy in (-8, -4, 0, 4, 8):
            own = 0
            cheb = 0
            for c, (x0, x1, y0, y1, blocks) in zip(cells, boxes):
                obx = (c["grid"][0] - (ox + sx)) // 4
                oby = (c["grid"][1] - (oy + sy)) // 4
                own += (obx, oby) in blocks
                cheb = max(cheb, obx - x0, x1 - obx, oby - y0, y1 - oby)
            rows.append(dict(origin=(ox + sx, oy + sy), own_block_set=own, max_block_chebyshev=cheb))
    return rows


# ----------------------------------------------------------------------------- MHDT units

def vhgt_heights(d):
    """33 x 33 terrain heights [y][x] in game units from a LAND VHGT payload (offset f32, 33 x 33
    int8 deltas: a row's first value is relative to the previous row's first value, the others
    to their left neighbour; everything in units of 8)."""
    off = struct.unpack_from("<f", d, 0)[0]
    deltas = struct.unpack_from("<1089b", d, 4)
    h = [[0.0] * 33 for _ in range(33)]
    row = off
    for r in range(33):
        row += deltas[r * 33]
        v = row
        h[r][0] = v * 8
        for c in range(1, 33):
            v += deltas[r * 33 + c]
            h[r][c] = v * 8
    return h


def tamriel_cells(path, wanted):
    """Skyrim.esm (or any plugin holding Tamriel, form id 0x3C): grid -> (MHDT bytes, VHGT
    heights) for the wanted grids, reading only the Tamriel world-children group."""
    buf = open(path, "rb").read()
    size = struct.unpack_from("<I", buf, 4)[0]
    out = {}
    o = REC_HEADER + size
    while o + REC_HEADER <= len(buf):
        gsize, label, gtype = struct.unpack_from("<I4si", buf, o + 4)
        if label == b"WRLD":
            break
        o += gsize
    else:
        return out
    end = o + gsize
    o += REC_HEADER
    while o + REC_HEADER <= end:
        if buf[o:o + 4] == b"GRUP":
            gs, lab, gt = struct.unpack_from("<I4si", buf, o + 4)
            if gt == 1 and struct.unpack("<I", lab)[0] == 0x3C:
                _walk_world(buf, o + REC_HEADER, o + gs, wanted, out)
            o += gs
        else:
            o += REC_HEADER + struct.unpack_from("<I", buf, o + 4)[0]
    return out


def _walk_world(buf, o, end, wanted, out):
    cur = None
    while o + REC_HEADER <= end:
        if buf[o:o + 4] == b"GRUP":
            gs, lab, gt = struct.unpack_from("<I4si", buf, o + 4)
            if gt in (4, 5):
                _walk_world(buf, o + REC_HEADER, o + gs, wanted, out)
            elif gt in (6, 8, 9) and cur is not None:
                _walk_land(buf, o + REC_HEADER, o + gs, cur, out)
            o += gs
            continue
        sig = buf[o:o + 4]
        rsize, flags, fid = struct.unpack_from("<III", buf, o + 4)
        cur = None
        if sig == b"CELL":
            grid = mh = None
            for s, d in subrecords(record_data(buf, o, rsize, flags)):
                if s == "XCLC" and len(d) >= 8:
                    grid = struct.unpack("<ii", d[:8])
                elif s == "MHDT":
                    mh = d
            if grid in wanted:
                out[grid] = dict(mhdt=mh, vhgt=None)
                cur = grid
        o += REC_HEADER + rsize


def _walk_land(buf, o, end, grid, out):
    while o + REC_HEADER <= end:
        if buf[o:o + 4] == b"GRUP":
            gs = struct.unpack_from("<I", buf, o + 4)[0]
            _walk_land(buf, o + REC_HEADER, o + gs, grid, out)
            o += gs
            continue
        sig = buf[o:o + 4]
        rsize, flags, fid = struct.unpack_from("<III", buf, o + 4)
        if sig == b"LAND":
            for s, d in subrecords(record_data(buf, o, rsize, flags)):
                if s == "VHGT":
                    out[grid]["vhgt"] = vhgt_heights(d)
        o += REC_HEADER + rsize


def mhdt_units(cells, header, other_plugins, skyrim_esm):
    """For the golden cells that are new records of DynDOLOD.esm without MHDT there (child-world
    cell copies, so no plugin holds an MHDT for them), compare the golden MHDT with (a) the
    parent Tamriel cell's MHDT at the same grid (copied or computed?) and (b) the Tamriel LAND
    heights there under candidate byte units and orders."""
    esm_path = next((p for p in other_plugins if p.replace("\\", "/").lower().endswith("/dyndolod.esm")), None)
    if esm_path is None or "DynDOLOD.esm" not in header["masters"]:
        print("  (needs --other-plugin DynDOLOD.esm of the same run)")
        return None
    esm = read_plugin(esm_path)
    esm_new = {}
    for w in esm["worlds"]:
        for c in w["cells"]:
            if (c["form_id"] >> 24) >= len(esm["header"]["masters"]) and "grid" in c:
                esm_new[c["form_id"] & 0xFFFFFF] = c
    idx = header["masters"].index("DynDOLOD.esm")
    computed = {}
    for c in cells:
        if (c["form_id"] >> 24) != idx or "mhdt" not in c:
            continue
        src = esm_new.get(c["form_id"] & 0xFFFFFF)
        if src is not None and "mhdt" not in src:
            computed[c["grid"]] = c["mhdt"]
    tam = tamriel_cells(skyrim_esm, set(computed))
    same = sum(1 for g, m in computed.items() if g in tam and tam[g]["mhdt"] == m)
    fits = OrderedDict()
    offsets = Counter()
    n = 0
    for g, m in computed.items():
        t = tam.get(g)
        if not t or t["vhgt"] is None:
            continue
        n += 1
        h = t["vhgt"]
        off = struct.unpack_from("<f", m, 0)[0]
        b = m[4:]
        tmin = min(min(r) for r in h)
        offsets["offset == terrain min rounded up to 128" if off == math.ceil(tmin / 128) * 128 else
                ("offset above terrain min" if off > tmin else "offset below terrain min")] += 1
        for unit in (8, 110, 128):
            for order in ("x-major", "y-major"):
                cnt = fits.setdefault(f"unit {unit}, {order}", Counter())
                for k in range(1024):
                    x, y = (k // 32, k % 32) if order == "x-major" else (k % 32, k // 32)
                    quad = max(h[y][x], h[y][x + 1], h[y + 1][x], h[y + 1][x + 1])
                    need = math.ceil((quad - off) / unit)
                    cnt["byte >= need"] += b[k] >= need
                    cnt["byte < need"] += b[k] < need
                    cnt["equal"] += b[k] == need
    print(f"  golden cells that are DynDOLOD.esm records without MHDT anywhere (computed by the reference): {len(computed)}")
    print(f"  of those, identical to the parent Tamriel cell's MHDT in {skyrim_esm}: {same} (copied) vs {len(computed) - same} different (computed)")
    print(f"  cells compared with the Tamriel LAND there: {n}; offsets: {dict(offsets)}")
    print("  byte >= ceil((max terrain height of the quad - offset) / unit), per unit and byte order:")
    for name, cnt in fits.items():
        print(f"    {name}: {dict(cnt)}")
    return dict(computed_cells=len(computed), identical_to_parent=same, compared=n,
                offsets={k: v for k, v in offsets.items()}, fits={k: dict(v) for k, v in fits.items()})


# ----------------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("esp", help="golden Occlusion.esp")
    ap.add_argument("--origin", default="-96,-96", help="lodsettings south-west cell x,y (Tamriel: -96,-96)")
    ap.add_argument("--stride", type=int, default=256, help="lodsettings stride in cells (Tamriel: 256)")
    ap.add_argument("--radius", type=int, default=100, help="OcclusionRadius in cells used for the run")
    ap.add_argument("--sample", type=int, default=40, help="cells per candidate in the first structure pass")
    ap.add_argument("--other-plugin", action="append", default=[],
                    help="plugin of the same run (DynDOLOD.esm/.esp) to count CELLs it adds without MHDT")
    ap.add_argument("--skyrim-esm", help="Skyrim.esm: with --other-plugin DynDOLOD.esm, test the MHDT byte unit on the cells the reference computed")
    ap.add_argument("--json", help="write the measured facts here")
    a = ap.parse_args()
    origin = tuple(int(v) for v in a.origin.split(","))
    stride = a.stride
    nb = stride // 4
    total = sum(n * n for n in level_sizes(nb))

    pl = read_plugin(a.esp)
    h = pl["header"]
    facts = OrderedDict(plugin=a.esp, bytes=pl["bytes"], header=h, origin=origin, stride=stride,
                        radius=a.radius, blocks_per_side=nb, levels=len(level_sizes(nb)),
                        stream_bits=total)
    print(f"{a.esp}: {pl['bytes']} bytes")
    print(f"  header: ESL {h['esl']}, master {h['master']}, HEDR {h.get('version'):.2f} records {h.get('records')} "
          f"next 0x{h.get('next_object_id', 0):X}, author {h.get('author')!r}, description {h.get('description')!r}, "
          f"INCC {h.get('incc')}")
    print(f"  masters ({len(h['masters'])}): {', '.join(h['masters'])}")

    # Worldspaces and cells.
    cells = []
    worlds = []
    for w in pl["worlds"]:
        tv = [c for c in w["cells"] if "tvdt" in c and "grid" in c]
        xs = [c["grid"][0] for c in tv]
        ys = [c["grid"][1] for c in tv]
        info = OrderedDict(edid=w.get("edid"), form_id=f"{w['form_id']:08X}",
                           parent=f"{w['parent']:08X}" if "parent" in w else None, pnam=w.get("pnam"),
                           dnam=w.get("dnam"), nam4=w.get("nam4"), data_flags=w.get("data_flags"),
                           subrecords=w["subs"], has_rnam="RNAM" in w["subs"], has_mhdt="MHDT" in w["subs"],
                           ofst_bytes=w.get("ofst_bytes"), cells=len(w["cells"]), cells_with_tvdt=len(tv),
                           cells_with_mhdt=sum("mhdt" in c for c in w["cells"]),
                           compressed_cells=sum(bool(c["flags"] & COMPRESSED) for c in w["cells"]),
                           grid_x=(min(xs), max(xs)) if xs else None, grid_y=(min(ys), max(ys)) if ys else None)
        worlds.append(info)
        cells.extend(tv)
        print(f"  WRLD {info['edid']} [{info['form_id']}] parent {info['parent']} PNAM {info['pnam']} "
              f"DNAM {info['dnam']} NAM4 {info['nam4']} DATA 0x{(info['data_flags'] or 0):02X}: "
              f"{info['cells']} CELL, {info['cells_with_tvdt']} with TVDT, {info['cells_with_mhdt']} with MHDT, "
              f"{info['compressed_cells']} compressed, x {info['grid_x']} y {info['grid_y']}; "
              f"RNAM {info['has_rnam']}, MHDT {info['has_mhdt']}, OFST {info['ofst_bytes']} bytes")
    facts["worlds"] = worlds
    facts["cells_with_tvdt"] = len(cells)

    # Subrecord order of the copied CELLs against xEdit's definition order.
    rank = {s: i for i, s in enumerate(CELL_ORDER)}
    in_order = sum(all(rank.get(x, 99) <= rank.get(y, 99) for x, y in zip(c["subs"], c["subs"][1:])) for c in cells)
    unknown = Counter(s for c in cells for s in c["subs"] if s not in rank)
    facts["cells_in_xedit_order"] = in_order
    facts["cell_subrecords_outside_xedit_list"] = dict(unknown)
    print(f"  CELL subrecords in xEdit's CELL order: {in_order}/{len(cells)}; signatures outside the list: {dict(unknown)}")

    # Sizes.
    tv_sizes = Counter(len(c["tvdt"]) for c in cells)
    mh_sizes = Counter(len(c["mhdt"]) for c in cells if "mhdt" in c)
    facts["tvdt_sizes"] = dict(tv_sizes)
    facts["mhdt_sizes"] = dict(mh_sizes)
    print(f"  TVDT sizes: {dict(tv_sizes)}; MHDT sizes: {dict(mh_sizes)}")
    first_u32 = Counter(struct.unpack("<I", c["tvdt"][:4])[0] for c in cells)
    last_byte = Counter(c["tvdt"][-1] for c in cells)
    facts["tvdt_first_u32"] = {str(k): v for k, v in first_u32.items()}
    facts["tvdt_last_byte"] = {str(k): v for k, v in last_byte.items()}
    print(f"  TVDT first u32 values: {dict(first_u32)}; last byte values: {dict(last_byte)}")
    if list(tv_sizes) != [684]:
        print("  !! expected every TVDT to be 684 bytes")
    if 684 * 8 < total:
        raise SystemExit("stride too large for a 684-byte TVDT")

    # 2. Structure.
    print(f"\nSTRUCTURE: {total} stream bits ({' + '.join(str(n * n) for n in level_sizes(nb))}) in 684 x 8 = {684 * 8} bits")
    cands = structure_candidates(cells, nb, total, a.sample)
    survivors = []
    for name, r in cands.items():
        flag = "OK " if r["bad_nodes"] == 0 else "   "
        print(f"  {flag}{name}: {r['bad_cells']}/{r['sample_cells']} sample cells with a parent != OR(children), {r['bad_nodes']} bad nodes")
        if r["bad_nodes"] == 0:
            survivors.append(name)
    facts["structure_candidates"] = {n: dict(sample_cells=r["sample_cells"], bad_cells=r["bad_cells"], bad_nodes=r["bad_nodes"]) for n, r in cands.items()}
    facts["structure_survivors"] = survivors
    if not survivors:
        print("  !! no candidate layout satisfies parent == OR(children); the layout is not one of the tested ones")
        if a.json:
            json.dump(facts, open(a.json, "w"), indent=1, default=str)
        return 1
    final = []
    for name in survivors:
        full = full_structure_check(cells, cands[name])
        print(f"  all cells, {name}: {full['bad_cells']}/{full['cells']} cells with a bad node, "
              f"{full['bad_nodes']} of {full['cells'] * full['internal_nodes_per_cell']} parent/child relations wrong")
        facts.setdefault("structure_full", {})[name] = full
        if full["bad_nodes"] == 0:
            final.append(name)
    # A post-order stream read backwards is a pre-order stream read forwards with the siblings in
    # reverse order: the same bit <-> node assignment described twice. Keep the pre-order,
    # forward description (bit position 0 is the whole-grid bit) as the canonical one.
    def opposite(name):
        if name.startswith("post-order"):
            return name.replace("post-order (children, then parent)", "pre-order (parent, then children)") \
                .replace("forward", "@").replace("backward", "forward").replace("@", "backward")
        return None
    equivalent = len(final) == 2 and any(opposite(n) == m for n in final for m in final)
    if equivalent:
        print(f"  the two survivors are one assignment of bits to nodes described twice (reversed stream, reversed sibling order)")
    elif len(final) != 1:
        print(f"  !! {len(final)} structure candidates survive the full check: {final}")
    facts["structure_equivalent_pair"] = equivalent
    best = next((n for n in final if n.startswith("pre-order")), final[0] if final else survivors[0])
    cand = cands[best]
    leaves = leaf_streams(cells, cand, nb)
    stray = sum(lv["stray_bits"] for lv in leaves)
    roots = sum(lv["root"] for lv in leaves)
    set_counts = [len(lv["leaves"]) for lv in leaves]
    facts["layout"] = best
    # AnvilLOD's TvdtData.cs description (6 lead bits, post-order, forward): do its leaf bits land
    # on the same positions as the survivor's? (Its internal-node bits are tested above as a
    # candidate of their own.)
    anvil_name = "post-order (children, then parent); lsb-first; forward; 6 lead bits"
    if anvil_name in cands:
        leaves_best = {m: cand["pos"][k] for k, (l, m) in enumerate(cand["nodes"]) if l == 0}
        ap = cands[anvil_name]
        leaves_anvil = {m: ap["pos"][k] for k, (l, m) in enumerate(ap["nodes"]) if l == 0}
        same_leaf_bits = sum(leaves_best[m] == leaves_anvil[m] for m in leaves_best)
        facts["anvillod_description"] = dict(leaf_bits_identical=same_leaf_bits, leaves=len(leaves_best),
                                             bad_nodes_in_sample=ap["bad_nodes"], sample_cells=ap["sample_cells"])
        print(f"  AnvilLOD TvdtData.cs description (6 lead bits + post-order): leaf bit positions identical for "
              f"{same_leaf_bits}/{len(leaves_best)} leaves; its internal-node bits: {ap['bad_nodes']} parent != OR(children) "
              f"in {ap['sample_cells']} sample cells")
    facts["bits_set_outside_stream"] = stray
    facts["root_bit_set"] = roots
    facts["set_leaf_blocks"] = dict(min=min(set_counts), max=max(set_counts),
                                    mean=round(sum(set_counts) / len(set_counts), 1))
    pos = cand["pos"]
    print(f"  layout: {best}")
    print(f"  stream index 0 at byte {pos[0] >> 3} bit {pos[0] & 7}; stream index {total - 1} at byte {pos[-1] >> 3} bit {pos[-1] & 7}")
    print(f"  bits set outside the {total}-bit stream (the {684 * 8 - total} spare bits): {stray} over all cells")
    print(f"  root (whole grid) bit set: {roots}/{len(cells)} cells")
    print(f"  set 4x4 blocks per cell: min {min(set_counts)}, max {max(set_counts)}, mean {facts['set_leaf_blocks']['mean']} of {nb * nb}")

    # 3. Geometry.
    print(f"\nGEOMETRY: origin {origin}, stride {stride}, radius {a.radius}")
    geo = geometry(cells, leaves, nb, origin, stride, a.radius)
    geo_ok = []
    for name, r in geo.items():
        ok = r["own_block_set"] == r["cells"] and r["blocks_beyond_radius"] == 0
        dist = (f"blocks beyond the radius {r['blocks_beyond_radius']}, farthest set block: nearest cell "
                f"{r['max_nearest_cell_distance']}, farthest cell {r['max_farthest_cell_distance']} cells"
                if r["blocks_beyond_radius"] is not None else "(distances not computed)")
        print(f"  {'OK ' if ok else '   '}{name}: own block set {r['own_block_set']}/{r['cells']}, {dist}")
        if ok:
            geo_ok.append(name)
    facts["geometry_candidates"] = {n: {k: v for k, v in r.items() if k != "table"} for n, r in geo.items()}
    facts["geometry_survivors"] = geo_ok
    if len(geo_ok) != 1:
        print(f"  !! {len(geo_ok)} geometry candidates survive: {geo_ok}")
    gbest = geo_ok[0] if geo_ok else max(geo, key=lambda n: geo[n]["own_block_set"])
    facts["geometry"] = gbest
    print(f"  mapping: {gbest}")
    # The first stream entries under the canonical description, as (bit position, level, block).
    table = geo[gbest]["table"]
    sizes = level_sizes(nb)
    first = []
    for k in range(12):
        l, m = cand["nodes"][k]
        # block of a level-l node: its first leaf's block, as a (level-l block) coordinate
        leaf_m = m << (2 * l)
        bx, by = table[leaf_m]
        side = 4 << l
        first.append(dict(bit=pos[k], level=l, cells_per_side=side, block=(bx * 4 // side, by * 4 // side),
                          cells=((origin[0] + (bx * 4 // side) * side, origin[1] + (by * 4 // side) * side))))
    facts["first_stream_entries"] = first
    print("  first stream entries (bit position: level, block side in cells, block index, south-west cell):")
    for e in first:
        print(f"    bit {e['bit']}: level {e['level']}, {e['cells_per_side']}x{e['cells_per_side']} cells, block {e['block']}, from cell {e['cells']}")
    shifts = origin_shift_table(cells, leaves, geo[gbest]["table"], nb, origin, a.radius)
    facts["origin_shifts"] = shifts
    limit = -(-a.radius // 4) + 1
    print(f"  origin shifted by whole blocks (own block set / max Chebyshev block distance; expect {len(cells)} / <= {limit} only at the lodsettings origin):")
    for row in shifts:
        mark = " <- lodsettings" if row["origin"] == origin else ""
        print(f"    origin {row['origin']}: {row['own_block_set']} / {row['max_block_chebyshev']}{mark}")

    # Observed extents.
    table = geo[gbest]["table"]
    allx = [table[m][0] for lv in leaves for m in lv["leaves"]]
    ally = [table[m][1] for lv in leaves for m in lv["leaves"]]
    ext = dict(block_x=(min(allx), max(allx)), block_y=(min(ally), max(ally)),
               cell_x=(origin[0] + 4 * min(allx), origin[0] + 4 * max(allx) + 3),
               cell_y=(origin[1] + 4 * min(ally), origin[1] + 4 * max(ally) + 3))
    facts["set_block_extent"] = ext
    print(f"  set blocks span blocks x {ext['block_x']} y {ext['block_y']} = cells x {ext['cell_x']} y {ext['cell_y']} (grid: {origin[0]}..{origin[0] + stride - 1})")

    # 4. MHDT.
    mh = [c for c in cells if "mhdt" in c]
    offsets = [struct.unpack("<f", c["mhdt"][:4])[0] for c in mh]
    mult128 = sum(o % 128 == 0 for o in offsets)
    maxbyte = max(max(c["mhdt"][4:]) for c in mh) if mh else None
    facts["mhdt"] = dict(cells=len(mh), offset_multiple_of_128=mult128, offset_min=min(offsets), offset_max=max(offsets),
                         max_byte=maxbyte)
    print(f"\nMHDT: {len(mh)}/{len(cells)} TVDT cells carry MHDT; offsets multiple of 128: {mult128}/{len(mh)}; "
          f"offset range {min(offsets)}..{max(offsets)}; max byte {maxbyte}")
    if a.other_plugin:
        by_id = {}
        for p in a.other_plugin:
            op = read_plugin(p)
            for w in op["worlds"]:
                for c in w["cells"]:
                    # Map the other plugin's form id into this plugin's master space by file name.
                    master = op["header"]["masters"][c["form_id"] >> 24] if (c["form_id"] >> 24) < len(op["header"]["masters"]) else p.replace("\\", "/").rsplit("/", 1)[-1]
                    by_id[(master.lower(), c["form_id"] & 0xFFFFFF)] = ("mhdt" in c, p)
        new_here = Counter()
        for c in mh:
            idx = c["form_id"] >> 24
            master = h["masters"][idx].lower() if idx < len(h["masters"]) else "?"
            k = (master, c["form_id"] & 0xFFFFFF)
            if k in by_id:
                has, p = by_id[k]
                new_here[(p.replace("\\", "/").rsplit("/", 1)[-1], "override has MHDT" if has else "override without MHDT")] += 1
        facts["mhdt_other_plugins"] = {f"{k[0]}: {k[1]}": v for k, v in new_here.items()}
        for k, v in sorted(new_here.items()):
            print(f"  cells whose record in {k[0]} {k[1]}: {v}")
    if a.skyrim_esm:
        print("MHDT units (cells the reference computed vs. the Tamriel LAND of Skyrim.esm):")
        facts["mhdt_units"] = mhdt_units(cells, h, a.other_plugin, a.skyrim_esm)

    if a.json:
        json.dump(facts, open(a.json, "w"), indent=1, default=str)
        print(f"\nwrote {a.json}")
    ok = ((len(final) == 1 or equivalent) and stray == 0 and roots == len(cells) and len(geo_ok) == 1)
    print("\nPASS" if ok else "\nFAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
