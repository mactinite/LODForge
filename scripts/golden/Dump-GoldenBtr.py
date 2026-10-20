#!/usr/bin/env python3
"""Dump and measure golden terrain LOD meshes (.btr) and terrain underside NIFs.

A stand-alone reader for the handful of block types terrain LOD files use, written from the
niftools NIF format specification (nif.xml, niftoolsxml 0.10.0.0, commit 028cb35b 2023-06-17):
NiNode / BSFadeNode / BSMultiBoundNode, BSTriShape / BSSubIndexTriShape (BS version 100 vertex
layout), BSLightingShaderProperty, BSShaderTextureSet, BSMultiBound, BSMultiBoundAABB. Every
other block type is skipped using the block sizes in the header.

It is a measurement tool for docs/re/terrain-lod-spec.md: it contains no generation logic.

Usage:
  Dump-GoldenBtr.py dump <file.btr|nif> [--verts N]      full structure of one file
  Dump-GoldenBtr.py stats <dir> [--log LODGen_log.txt]  per-level statistics over a folder of .btr
  Dump-GoldenBtr.py seams <dir>                          border-vertex agreement of neighbouring quads
  Dump-GoldenBtr.py underside <World_Underside.nif>      per-quad summary of an underside NIF
  Dump-GoldenBtr.py compare <dir A> <dir B> [--tol T]    tolerant BTR comparison of two folders

Run with the real interpreter on this machine:
  C:\\Users\\macti\\AppData\\Local\\Python\\bin\\python3.exe -I scripts\\golden\\Dump-GoldenBtr.py ...
"""
import json
import math
import os
import re
import statistics
import struct
import sys
from collections import Counter, defaultdict


# ----------------------------------------------------------------------------------------------
# NIF reading (SSE: 20.2.0.7 / user 12 / BS 100)
# ----------------------------------------------------------------------------------------------

def half_to_float(h):
    s = (h >> 15) & 1
    e = (h >> 10) & 0x1F
    m = h & 0x3FF
    if e == 0:
        v = (m / 1024.0) * 2.0 ** -14
    elif e == 31:
        v = float("inf") if m == 0 else float("nan")
    else:
        v = (1.0 + m / 1024.0) * 2.0 ** (e - 15)
    return -v if s else v


class Reader:
    def __init__(self, data):
        self.d = data
        self.p = 0

    def u8(self):
        v = self.d[self.p]
        self.p += 1
        return v

    def u16(self):
        (v,) = struct.unpack_from("<H", self.d, self.p)
        self.p += 2
        return v

    def u32(self):
        (v,) = struct.unpack_from("<I", self.d, self.p)
        self.p += 4
        return v

    def i32(self):
        (v,) = struct.unpack_from("<i", self.d, self.p)
        self.p += 4
        return v

    def u64(self):
        (v,) = struct.unpack_from("<Q", self.d, self.p)
        self.p += 8
        return v

    def f32(self):
        (v,) = struct.unpack_from("<f", self.d, self.p)
        self.p += 4
        return v

    def f32s(self, n):
        v = struct.unpack_from("<%df" % n, self.d, self.p)
        self.p += 4 * n
        return list(v)

    def sized_string(self):
        n = self.u32()
        s = self.d[self.p : self.p + n].decode("latin-1")
        self.p += n
        return s

    def short_string(self):
        n = self.u8()
        s = self.d[self.p : self.p + n].decode("latin-1").rstrip("\0")
        self.p += n
        return s

    def ref(self):
        v = self.i32()
        return None if v < 0 else v

    def refs(self):
        return [self.ref() for _ in range(self.u32())]


def read_header(r):
    end = r.d.index(b"\n", r.p)
    h = {"header_line": r.d[r.p:end].decode("latin-1")}
    r.p = end + 1
    h["version"] = r.u32()
    h["endian"] = r.u8()
    h["user_version"] = r.u32()
    h["num_blocks"] = r.u32()
    h["bs_version"] = r.u32()
    h["author"] = r.short_string()
    h["process_script"] = r.short_string()
    h["export_script"] = r.short_string()
    n = r.u16()
    h["block_types"] = [r.sized_string() for _ in range(n)]
    h["block_type_index"] = [r.u16() for _ in range(h["num_blocks"])]
    h["block_sizes"] = [r.u32() for _ in range(h["num_blocks"])]
    ns = r.u32()
    h["max_string_length"] = r.u32()
    h["strings"] = [r.sized_string() for _ in range(ns)]
    h["groups"] = [r.u32() for _ in range(r.u32())]
    return h


def object_net(r, strings):
    i = r.i32()
    return {"name": None if i < 0 else strings[i], "extra_data": r.refs(), "controller": r.ref()}


def av_object(r, strings):
    o = object_net(r, strings)
    o["flags"] = r.u32()
    o["translation"] = r.f32s(3)
    o["rotation"] = [r.f32s(3) for _ in range(3)]
    o["scale"] = r.f32()
    o["collision"] = r.ref()
    return o


def node(r, strings, multibound):
    n = av_object(r, strings)
    n["children"] = r.refs()
    n["effects"] = r.refs()
    if multibound:
        n["multi_bound"] = r.ref()
        n["culling_mode"] = r.u32()
    return n


VA_VERTEX, VA_UVS, VA_UVS2, VA_NORMALS, VA_TANGENTS, VA_COLORS, VA_SKINNED, VA_LAND, VA_EYE, VA_FULL = (
    0x001, 0x002, 0x004, 0x008, 0x010, 0x020, 0x040, 0x080, 0x100, 0x400,
)


def tri_shape(r, strings, sub_index):
    s = av_object(r, strings)
    s["bound_center"] = r.f32s(3)
    s["bound_radius"] = r.f32()
    s["skin"] = r.ref()
    s["shader_property"] = r.ref()
    s["alpha_property"] = r.ref()
    desc = r.u64()
    s["vertex_desc"] = desc
    attrs = (desc >> 44) & 0xFFF
    stride = (desc & 0xF) * 4
    s["vertex_attributes"] = attrs
    s["vertex_stride"] = stride
    nt = r.u16()
    nv = r.u16()
    s["num_triangles"] = nt
    s["num_vertices"] = nv
    data_size = r.u32()
    s["data_size"] = data_size
    pos, uvs, nrm, tan, col = [], [], [], [], []
    if data_size > 0:
        for _ in range(nv):
            start = r.p
            if attrs & VA_VERTEX:
                pos.append(r.f32s(3))
                r.f32()  # bitangent X (tangents) / unused
            if attrs & VA_UVS:
                uvs.append([half_to_float(r.u16()), half_to_float(r.u16())])
            if attrs & VA_NORMALS:
                nrm.append([r.u8(), r.u8(), r.u8()])
                r.u8()
            if attrs & VA_TANGENTS:
                tan.append([r.u8(), r.u8(), r.u8()])
                r.u8()
            if attrs & VA_COLORS:
                col.append([r.u8(), r.u8(), r.u8(), r.u8()])
            if attrs & VA_SKINNED:
                r.p += 8 + 4
            if attrs & VA_EYE:
                r.p += 4
            r.p = start + stride
        tris = []
        for _ in range(nt):
            tris.append([r.u16(), r.u16(), r.u16()])
        s["triangles"] = tris
    else:
        s["triangles"] = []
    s["positions"], s["uvs"], s["normals"], s["tangents"], s["colors"] = pos, uvs, nrm, tan, col
    s["particle_data_size"] = r.u32()
    if s["particle_data_size"] > 0:
        raise ValueError("particle data not supported")
    if sub_index:
        segs = []
        for _ in range(r.u32()):
            segs.append({"flags": r.u8(), "index": r.u32(), "num_tris": r.u32()})
        s["segments"] = segs
    return s


def lighting_shader(r, strings):
    s = {"shader_type": r.u32()}
    s.update(object_net(r, strings))
    s["flags1"] = r.u32()
    s["flags2"] = r.u32()
    s["uv_offset"] = r.f32s(2)
    s["uv_scale"] = r.f32s(2)
    s["texture_set"] = r.ref()
    s["emissive_color"] = r.f32s(3)
    s["emissive_multiple"] = r.f32()
    s["texture_clamp_mode"] = r.u32()
    s["alpha"] = r.f32()
    s["refraction_strength"] = r.f32()
    s["glossiness"] = r.f32()
    s["specular_color"] = r.f32s(3)
    s["specular_strength"] = r.f32()
    s["lighting_effect_1"] = r.f32()
    s["lighting_effect_2"] = r.f32()
    # type-specific tails are not needed for terrain (type 0 / 18 have none)
    return s


def read_nif(path):
    data = open(path, "rb").read()
    r = Reader(data)
    h = read_header(r)
    blocks = []
    for i in range(h["num_blocks"]):
        t = h["block_types"][h["block_type_index"][i]]
        start = r.p
        size = h["block_sizes"][i]
        b = {"index": i, "type": t}
        try:
            if t in ("NiNode", "BSFadeNode"):
                b.update(node(r, h["strings"], False))
            elif t == "BSMultiBoundNode":
                b.update(node(r, h["strings"], True))
            elif t == "BSTriShape":
                b.update(tri_shape(r, h["strings"], False))
            elif t == "BSSubIndexTriShape":
                b.update(tri_shape(r, h["strings"], True))
            elif t == "BSLightingShaderProperty":
                b.update(lighting_shader(r, h["strings"]))
            elif t == "BSShaderTextureSet":
                b["textures"] = [r.sized_string() for _ in range(r.u32())]
            elif t == "BSMultiBound":
                b["data"] = r.ref()
            elif t == "BSMultiBoundAABB":
                b["position"] = r.f32s(3)
                b["extent"] = r.f32s(3)
            else:
                b["raw"] = True
        except Exception as e:  # noqa: BLE001
            b["error"] = str(e)
        if r.p - start != size:
            b["size_mismatch"] = (r.p - start, size)
        r.p = start + size
        blocks.append(b)
    roots = r.refs()
    return {"path": path, "header": h, "blocks": blocks, "roots": roots, "file_size": len(data)}


# ----------------------------------------------------------------------------------------------
# BTR model: root multibound node, land shapes, water node
# ----------------------------------------------------------------------------------------------

NAME_RE = re.compile(r"^(?P<world>.+?)\.(?P<level>\d+)\.(?P<x>-?\d+)\.(?P<y>-?\d+)(?:\.(?P<season>[A-Z]+))?\.btr$", re.I)


def parse_name(path):
    m = NAME_RE.match(os.path.basename(path))
    if not m:
        return None
    return {"world": m["world"], "level": int(m["level"]), "x": int(m["x"]), "y": int(m["y"]), "season": m["season"]}


def aabb_of(nif, mb_ref):
    if mb_ref is None:
        return None
    mb = nif["blocks"][mb_ref]
    if mb.get("data") is None:
        return None
    a = nif["blocks"][mb["data"]]
    return {"position": a.get("position"), "extent": a.get("extent")}


def btr_model(nif):
    root = nif["blocks"][nif["roots"][0]]
    m = {"root": root, "land": [], "water_node": None, "water": [], "shader": None, "textures": None}
    m["root_aabb"] = aabb_of(nif, root.get("multi_bound"))
    for c in root["children"]:
        b = nif["blocks"][c]
        if b["type"] in ("BSTriShape", "BSSubIndexTriShape"):
            m["land"].append(b)
            if b.get("shader_property") is not None and m["shader"] is None:
                sh = nif["blocks"][b["shader_property"]]
                m["shader"] = sh
                if sh.get("texture_set") is not None:
                    m["textures"] = nif["blocks"][sh["texture_set"]].get("textures")
        elif b["type"] == "BSMultiBoundNode":
            m["water_node"] = b
            m["water_aabb"] = aabb_of(nif, b.get("multi_bound"))
            for wc in b["children"]:
                m["water"].append(nif["blocks"][wc])
    return m


def border_pairs_depth(shape):
    """Mode of (max z - min z) over positions on the quad border that hold more than one vertex:
    the skirt depth in quad-local units."""
    groups = defaultdict(set)
    for p in shape["positions"]:
        x, y, z = p
        if x == 0.0 or x == 4096.0 or y == 0.0 or y == 4096.0:
            groups[(x, y)].add(z)
    depths = Counter()
    for zs in groups.values():
        if len(zs) > 1:
            depths[round(max(zs) - min(zs), 3)] += 1
    return depths


def crossing_triangles(shape, spacing):
    """Triangles whose XY projection straddles an interior grid line x = k*spacing or y = k*spacing."""
    n = 0
    lines = [k * spacing for k in range(1, int(4096 // spacing))]
    for t in shape["triangles"]:
        xs = [shape["positions"][i][0] for i in t]
        ys = [shape["positions"][i][1] for i in t]
        if any(min(xs) < L < max(xs) for L in lines) or any(min(ys) < L < max(ys) for L in lines):
            n += 1
    return n


def uv_error(shape):
    """Max deviation of the UVs from (x/4096, 1 - y/4096)."""
    e = 0.0
    for p, uv in zip(shape["positions"], shape["uvs"]):
        e = max(e, abs(uv[0] - p[0] / 4096.0), abs(uv[1] - (1.0 - p[1] / 4096.0)))
    return e


def bbox(points):
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    zs = [p[2] for p in points]
    return [min(xs), min(ys), min(zs)], [max(xs), max(ys), max(zs)]


# ----------------------------------------------------------------------------------------------
# Commands
# ----------------------------------------------------------------------------------------------

def fmt(v):
    if isinstance(v, float):
        return repr(v)
    if isinstance(v, list):
        return "[" + ", ".join(fmt(x) for x in v) + "]"
    return str(v)


def cmd_dump(path, nverts):
    nif = read_nif(path)
    h = nif["header"]
    print("file:", path, "(%d bytes)" % nif["file_size"])
    for k in ("header_line", "version", "endian", "user_version", "bs_version", "author", "process_script", "export_script", "block_types", "block_type_index", "block_sizes", "max_string_length", "strings", "groups"):
        v = h[k]
        if k == "version":
            v = "0x%08X" % v
        print("  %s: %s" % (k, v))
    print("  roots:", nif["roots"])
    for b in nif["blocks"]:
        print("block %d %s" % (b["index"], b["type"]))
        for k, v in b.items():
            if k in ("index", "type"):
                continue
            if k in ("positions", "uvs", "normals", "tangents", "colors", "triangles"):
                print("    %s: %d entries" % (k, len(v)))
                for e in v[:nverts]:
                    print("      ", fmt(e))
                continue
            if k in ("flags", "flags1", "flags2"):
                v = "0x%08X" % v
            if k == "vertex_desc":
                v = "0x%016X" % v
            if k == "vertex_attributes":
                v = "0x%03X" % v
            print("    %s: %s" % (k, fmt(v)))


def level_files(d):
    out = defaultdict(list)
    for root, _, files in os.walk(d):
        for f in files:
            n = parse_name(f)
            if n and not n["season"]:
                out[n["level"]].append((os.path.join(root, f), n))
    return out


def q(values):
    if not values:
        return "-"
    s = sorted(values)
    return "min %s / p50 %s / max %s / sum %s" % (s[0], s[len(s) // 2], s[-1], sum(values))


def parse_log(path):
    counts = {}
    for line in open(path, encoding="latin-1"):
        m = re.search(r"Finished LOD level (.+?\.(\d+)\.(-?\d+)\.(-?\d+))\.btr \[(\d+)/(\d+)\]", line)
        if m:
            counts[(int(m[2]), int(m[3]), int(m[4]))] = (int(m[5]), int(m[6]))
    return counts


def cmd_stats(d, log):
    logc = parse_log(log) if log else {}
    for level in sorted(level_files(d)):
        files = level_files(d)[level]
        verts, tris, wverts, wtris, wshapes, wsegs, sizes = [], [], [], [], [], [], []
        descs, flags, transforms, shaders, cull, wcull, names, auth, btypes = Counter(), Counter(), Counter(), Counter(), Counter(), Counter(), Counter(), Counter(), Counter()
        depths, crossing_cell, uverr, land_shapes, water_heights, seg_tris, seg_flags = Counter(), 0, 0.0, Counter(), Counter(), Counter(), Counter()
        zmin, zmax, xy_ok, aabb_ok, bound_ok = math.inf, -math.inf, 0, 0, 0
        log_land_eq, log_water_eq, log_n = 0, 0, 0
        wtypes = Counter()
        tex_pattern = Counter()
        for path, n in files:
            nif = read_nif(path)
            m = btr_model(nif)
            sizes.append(nif["file_size"])
            auth[nif["header"]["author"]] += 1
            btypes[tuple(nif["header"]["block_types"])] += 1
            names[tuple(nif["header"]["strings"])] += 1
            root = m["root"]
            cull[root.get("culling_mode")] += 1
            land_shapes[len(m["land"])] += 1
            s0 = m["land"][0]
            verts.append(s0["num_vertices"])
            tris.append(s0["num_triangles"])
            descs["0x%016X" % s0["vertex_desc"]] += 1
            flags["0x%08X" % s0["flags"]] += 1
            transforms[(tuple(s0["translation"]), s0["scale"])] += 1
            sh = m["shader"]
            if sh:
                shaders[(sh["shader_type"], "0x%08X" % sh["flags1"], "0x%08X" % sh["flags2"], tuple(sh["uv_offset"]), tuple(sh["uv_scale"]), tuple(sh["emissive_color"]), sh["emissive_multiple"], sh["texture_clamp_mode"], sh["alpha"], sh["refraction_strength"], sh["glossiness"], tuple(sh["specular_color"]), sh["specular_strength"], sh["lighting_effect_1"], sh["lighting_effect_2"], sh["name"])] += 1
            if m["textures"]:
                tex_pattern[tuple(re.sub(r"\.\d+\.-?\d+\.-?\d+", ".L.X.Y", t) for t in m["textures"])] += 1
            lo, hi = bbox(s0["positions"])
            zmin, zmax = min(zmin, lo[2]), max(zmax, hi[2])
            if lo[0] >= 0 and lo[1] >= 0 and hi[0] <= 4096 and hi[1] <= 4096:
                xy_ok += 1
            depths.update(border_pairs_depth(s0))
            crossing_cell += crossing_triangles(s0, 4096 / level)
            uverr = max(uverr, uv_error(s0))
            # root AABB vs land bbox (in world units = local * level)
            a = m["root_aabb"]
            if a:
                c = [(lo[i] + hi[i]) / 2 * level for i in range(3)]
                e = [(hi[i] - lo[i]) / 2 * level for i in range(3)]
                top = max(hi[2], 0.0)
                c2 = (lo[2] + top) / 2 * level
                e2 = (top - lo[2]) / 2 * level
                if all(abs(a["position"][i] - c[i]) < 1.0 and abs(a["extent"][i] - e[i]) < 1.0 for i in range(2)) and (abs(a["position"][2] - c2) < 1.0 and abs(a["extent"][2] - e2) < 1.0):
                    aabb_ok += 1
            # bound sphere: centre = bbox centre, radius = distance centre -> max corner?
            c = [(lo[i] + hi[i]) / 2 for i in range(3)]
            rad = math.sqrt(sum((c[i] - hi[i]) ** 2 for i in range(3)))
            if all(abs(s0["bound_center"][i] - c[i]) < 0.01 for i in range(3)) and abs(s0["bound_radius"] - rad) < 0.05:
                bound_ok += 1
            wt = 0
            if m["water_node"]:
                wcull[m["water_node"].get("culling_mode")] += 1
                wshapes.append(len(m["water"]))
                for w in m["water"]:
                    wtypes[w["type"]] += 1
                    wverts.append(w["num_vertices"])
                    wtris.append(w["num_triangles"])
                    wt += w["num_triangles"]
                    for z in set(round(p[2] * level, 2) for p in w["positions"]):
                        water_heights[z] += 1
                    if "segments" in w:
                        wsegs.append(len(w["segments"]))
                        for sg in w["segments"]:
                            seg_tris[sg["num_tris"]] += 1
                            seg_flags[sg["flags"]] += 1
            key = (level, n["x"], n["y"])
            if key in logc:
                log_n += 1
                if logc[key][0] == s0["num_triangles"]:
                    log_land_eq += 1
                if logc[key][1] == wt:
                    log_water_eq += 1
        print("=== LOD%d: %d files" % (level, len(files)))
        print("  file bytes:", q(sizes))
        print("  header author:", dict(auth))
        print("  block types (header order):", dict(btypes))
        print("  string table:", dict(names))
        print("  root culling mode:", dict(cull), " land shapes per file:", dict(land_shapes))
        print("  land shape 0 vertices:", q(verts))
        print("  land shape 0 triangles:", q(tris))
        print("  land vertex desc:", dict(descs), " flags:", dict(flags), " (translation, scale):", dict(transforms))
        print("  land local xy within 0..4096: %d/%d; local z range %s..%s" % (xy_ok, len(files), zmin, zmax))
        print("  skirt depth (local units, mode over border pairs):", depths.most_common(4))
        print("  triangles straddling an interior cell line (%g local units):" % (4096 / level), crossing_cell)
        print("  max |uv - (x/4096, 1-y/4096)|:", uverr)
        print("  root AABB == land bbox*level with max z clamped to >=0: %d/%d; bound sphere == bbox centre / dist to max corner: %d/%d" % (aabb_ok, len(files), bound_ok, len(files)))
        print("  shader (type, flags1, flags2, uv off, uv scale, emissive, emis mult, clamp, alpha, refr, gloss, spec, spec str, le1, le2, name):")
        for k, v in shaders.items():
            print("     %d x %s" % (v, k))
        print("  texture slots pattern:", dict(tex_pattern))
        print("  water: files with water node %d; water node culling %s; shape types %s" % (len(wshapes), dict(wcull), dict(wtypes)))
        print("  water shapes per file:", Counter(wshapes), " vertices:", q(wverts), " triangles:", q(wtris))
        print("  water segments per shape:", Counter(wsegs), " segment num_tris:", dict(seg_tris), " segment flags:", dict(seg_flags))
        print("  distinct water heights (world units) seen in %d (height: shapes):" % len(water_heights), sorted(water_heights.items())[:12], "...")
        if logc:
            print("  LODGen log: %d files matched; land triangle count equal %d; water triangle count equal %d" % (log_n, log_land_eq, log_water_eq))


def border_sets(shape, level, qx, qy):
    """World-space (coord along border, z) sets for the W/E/S/N borders, skirts dropped (max z per position)."""
    out = {"W": {}, "E": {}, "S": {}, "N": {}}
    ox, oy = qx * 4096.0, qy * 4096.0
    for p in shape["positions"]:
        x, y, z = p
        wz = z * level
        if x == 0.0:
            out["W"][oy + y * level] = max(out["W"].get(oy + y * level, -math.inf), wz)
        if x == 4096.0:
            out["E"][oy + y * level] = max(out["E"].get(oy + y * level, -math.inf), wz)
        if y == 0.0:
            out["S"][ox + x * level] = max(out["S"].get(ox + x * level, -math.inf), wz)
        if y == 4096.0:
            out["N"][ox + x * level] = max(out["N"].get(ox + x * level, -math.inf), wz)
    return out


def cmd_seams(d):
    by_level = level_files(d)
    for level in sorted(by_level):
        idx = {(n["x"], n["y"]): path for path, n in by_level[level]}
        cache = {}

        def sets(k):
            if k not in cache:
                nif = read_nif(idx[k])
                cache[k] = border_sets(btr_model(nif)["land"][0], level, k[0], k[1])
            return cache[k]

        pairs, positions, same_z, close_z, only_one = 0, 0, 0, 0, 0
        for (x, y) in sorted(idx):
            for side, nb, other in (("E", (x + level, y), "W"), ("N", (x, y + level), "S")):
                if nb not in idx:
                    continue
                a, b = sets((x, y))[side], sets(nb)[other]
                pairs += 1
                keys = set(a) | set(b)
                for k in keys:
                    positions += 1
                    if k in a and k in b:
                        if a[k] == b[k]:
                            same_z += 1
                        elif abs(a[k] - b[k]) <= 1.0:
                            close_z += 1
                    else:
                        only_one += 1
        print("LOD%d: %d adjacent pairs, %d border positions: z identical %d, within 1 unit %d, present on one side only %d" % (level, pairs, positions, same_z, close_z, only_one))


def cmd_underside(path):
    nif = read_nif(path)
    h = nif["header"]
    print("file:", path, "(%d bytes)" % nif["file_size"])
    print("  author:", h["author"], " block types:", h["block_types"], " strings:", h["strings"])
    root = nif["blocks"][nif["roots"][0]]
    print("  root:", root["type"], "name", root["name"], "flags 0x%08X" % root["flags"], "children", len(root["children"]))
    quads = []
    for c in root["children"]:
        n = nif["blocks"][c]
        aabb = aabb_of(nif, n.get("multi_bound"))
        for sc in n["children"]:
            s = nif["blocks"][sc]
            sh = nif["blocks"][s["shader_property"]] if s.get("shader_property") is not None else None
            tex = nif["blocks"][sh["texture_set"]]["textures"] if sh and sh.get("texture_set") is not None else None
            lo, hi = bbox(s["positions"]) if s["positions"] else (None, None)
            neg = sum(1 for t in s["triangles"] if ((s["positions"][t[1]][0] - s["positions"][t[0]][0]) * (s["positions"][t[2]][1] - s["positions"][t[0]][1]) - (s["positions"][t[1]][1] - s["positions"][t[0]][1]) * (s["positions"][t[2]][0] - s["positions"][t[0]][0])) < 0)
            quads.append((s["translation"], s["scale"]))
            print("  quad node %d: culling %s flags 0x%08X | shape %s flags 0x%08X translation %s scale %s verts %d tris %d desc 0x%016X bound %s r %s | cw-from-above tris %d | local bbox %s..%s | uv err %.4g" % (c, n.get("culling_mode"), n["flags"], s["type"], s["flags"], fmt(s["translation"]), s["scale"], s["num_vertices"], s["num_triangles"], s["vertex_desc"], fmt(s["bound_center"]), s["bound_radius"], neg, fmt(lo), fmt(hi), uv_error(s) if s["uvs"] else -1))
            if sh:
                print("      shader type %d flags1 0x%08X flags2 0x%08X clamp %d gloss %s spec %s str %s le1 %s le2 %s alpha %s emis %s x %s" % (sh["shader_type"], sh["flags1"], sh["flags2"], sh["texture_clamp_mode"], sh["glossiness"], fmt(sh["specular_color"]), sh["specular_strength"], sh["lighting_effect_1"], sh["lighting_effect_2"], sh["alpha"], fmt(sh["emissive_color"]), sh["emissive_multiple"]))
                print("      textures", tex)
            if aabb:
                print("      AABB position %s extent %s" % (fmt(aabb["position"]), fmt(aabb["extent"])))
    print("  quads (translation, scale):", quads)


def shape_sets(shape, tol):
    r = lambda v: round(v / tol) if tol > 0 else v
    verts = set((r(p[0]), r(p[1]), r(p[2])) for p in shape["positions"])
    tris = set()
    for t in shape["triangles"]:
        v = [tuple(r(c) for c in shape["positions"][i]) for i in t]
        m = min(range(3), key=lambda k: v[k])
        tris.add((v[m], v[(m + 1) % 3], v[(m + 2) % 3]))
    return verts, tris


def cmd_compare(a, b, tol):
    fa, fb = level_files(a), level_files(b)
    for level in sorted(set(fa) | set(fb)):
        ia = {(n["x"], n["y"]): p for p, n in fa.get(level, [])}
        ib = {(n["x"], n["y"]): p for p, n in fb.get(level, [])}
        missing = len(set(ia) ^ set(ib))
        vt, vm, tt, tm, ws_eq, aabb_eq, files = 0, 0, 0, 0, 0, 0, 0
        for k in sorted(set(ia) & set(ib)):
            ma, mb = btr_model(read_nif(ia[k])), btr_model(read_nif(ib[k]))
            files += 1
            va, ta = shape_sets(ma["land"][0], tol)
            vb, tb = shape_sets(mb["land"][0], tol)
            vt += max(len(va), len(vb))
            vm += len(va & vb)
            tt += max(len(ta), len(tb))
            tm += len(ta & tb)
            if len(ma["water"]) == len(mb["water"]) and all(x["num_triangles"] == y["num_triangles"] and x.get("segments") == y.get("segments") for x, y in zip(ma["water"], mb["water"])):
                ws_eq += 1
            ra, rb = ma["root_aabb"], mb["root_aabb"]
            if ra and rb and all(abs(ra["position"][i] - rb["position"][i]) <= tol * level + 1e-3 and abs(ra["extent"][i] - rb["extent"][i]) <= tol * level + 1e-3 for i in range(3)):
                aabb_eq += 1
        print("LOD%d: %d files compared (%d only on one side): land vertices %d/%d in common, land triangles %d/%d, water shapes+segments equal %d, root AABB within tol %d" % (level, files, missing, vm, vt, tm, tt, ws_eq, aabb_eq))


# ----------------------------------------------------------------------------------------------
# fit: how far the golden mesh sits from the source height grid
# ----------------------------------------------------------------------------------------------

CELL_RECORD = 16 + 4 + 33 * 33 + 3


def read_terrain_bin(path):
    """xLODGen's `LODGen_Terrain_<World>.bin` as observed: per cell x i32, y i32, water f32, flags i32,
    then the LAND VHGT subrecord bytes (offset f32, 33x33 signed deltas, 3 bytes padding).
    VHGT decoding per UESP (Skyrim_Mod:Mod_File_Format/LAND): heights are in units of 8; rows run
    south to north, columns west to east; each row's first value is relative to the previous row's
    first value, the rest to the previous column."""
    data = open(path, "rb").read()
    cells = {}
    for off in range(0, len(data) - len(data) % CELL_RECORD, CELL_RECORD):
        x, y, water, flags, base = struct.unpack_from("<iifif", data, off)
        deltas = struct.unpack_from("<1089b", data, off + 20)
        h = [[0.0] * 33 for _ in range(33)]  # h[row][col], row 0 = south
        row_start = base * 8.0
        for r in range(33):
            row_start += deltas[r * 33] * 8.0
            v = row_start
            h[r][0] = v
            for c in range(1, 33):
                v += deltas[r * 33 + c] * 8.0
                h[r][c] = v
        cells[(x, y)] = {"water": water, "flags": flags, "h": h}
    return cells


def quad_grid(cells, level, qx, qy):
    """Local-unit height grid of a quad (index [i][j] = grid x, grid y; one step = 128/level
    local units) and the per-grid-point water height, NaN where no cell was exported."""
    n = 32 * level + 1
    g = [[math.nan] * n for _ in range(n)]
    w = [[math.nan] * n for _ in range(n)]
    for cy in range(qy, qy + level):
        for cx in range(qx, qx + level):
            c = cells.get((cx, cy))
            if not c:
                continue
            ox, oy = (cx - qx) * 32, (cy - qy) * 32
            for r in range(33):
                for col in range(33):
                    g[ox + col][oy + r] = c["h"][r][col] / level
                    w[ox + col][oy + r] = c["water"] / level
    return g, w


def cmd_fit(d, bin_path, sample):
    cells = read_terrain_bin(bin_path)
    print("terrain bin: %d cells" % len(cells))
    for level, files in sorted(level_files(d).items()):
        files = sorted(files, key=lambda f: (f[1]["y"], f[1]["x"]))
        if sample and len(files) > sample:
            step = len(files) / sample
            files = [files[int(i * step)] for i in range(sample)]
        step_units = 128.0 / level
        per_quad, per_quad_dry, verts_on_grid, verts_off_grid, verts_off_height, budget = [], [], 0, 0, 0, 0
        for path, n in files:
            m = btr_model(read_nif(path))
            s = m["land"][0]
            g, w = quad_grid(cells, level, n["x"], n["y"])
            pos = s["positions"]
            # skirt vertices: a lower duplicate of a border vertex
            top = {}
            for p in pos:
                k = (p[0], p[1])
                top[k] = max(top.get(k, -math.inf), p[2])
            skirt = [p[2] < top[(p[0], p[1])] for p in pos]
            for p, sk in zip(pos, skirt):
                if sk:
                    continue
                gi, gj = p[0] / step_units, p[1] / step_units
                if abs(gi - round(gi)) > 1e-6 or abs(gj - round(gj)) > 1e-6:
                    verts_off_grid += 1
                    continue
                verts_on_grid += 1
                hz = g[int(round(gi))][int(round(gj))]
                if not math.isnan(hz) and abs(hz - p[2]) > 1e-3:
                    verts_off_height += 1
            worst, worst_dry = 0.0, 0.0
            for t in s["triangles"]:
                if any(skirt[i] for i in t):
                    continue
                a, b, c = (pos[i] for i in t)
                det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
                if det == 0:
                    continue
                i0, i1 = int(math.ceil(min(a[0], b[0], c[0]) / step_units)), int(math.floor(max(a[0], b[0], c[0]) / step_units))
                j0, j1 = int(math.ceil(min(a[1], b[1], c[1]) / step_units)), int(math.floor(max(a[1], b[1], c[1]) / step_units))
                for i in range(i0, i1 + 1):
                    x = i * step_units
                    for j in range(j0, j1 + 1):
                        y = j * step_units
                        l1 = ((b[0] - x) * (c[1] - y) - (c[0] - x) * (b[1] - y)) / det
                        l2 = ((c[0] - x) * (a[1] - y) - (a[0] - x) * (c[1] - y)) / det
                        l3 = 1.0 - l1 - l2
                        if l1 < -1e-9 or l2 < -1e-9 or l3 < -1e-9:
                            continue
                        hz = g[i][j]
                        if math.isnan(hz):
                            continue
                        z = l1 * a[2] + l2 * b[2] + l3 * c[2]
                        e = abs(hz - z)
                        worst = max(worst, e)
                        if hz >= w[i][j]:
                            worst_dry = max(worst_dry, e)
            per_quad.append(worst)
            per_quad_dry.append(worst_dry)
        s = sorted(per_quad)
        sd = sorted(per_quad_dry)
        pick = lambda v, f: v[min(len(v) - 1, int(f * len(v)))]
        print("LOD%d (%d quads sampled): non-skirt vertices on the source grid %d, off grid %d, height differs from source at %d" % (level, len(files), verts_on_grid, verts_off_grid, verts_off_height))
        print("  max |source - mesh| per quad, local units (x%d = world): all points min %.3g p50 %.3g p90 %.3g max %.3g | above-water points min %.3g p50 %.3g p90 %.3g max %.3g" % (level, s[0], pick(s, 0.5), pick(s, 0.9), s[-1], sd[0], pick(sd, 0.5), pick(sd, 0.9), sd[-1]))


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    cmd = argv[1]
    if cmd == "fit":
        sample = int(argv[argv.index("--sample") + 1]) if "--sample" in argv else 0
        cmd_fit(argv[2], argv[3], sample)
        return 0
    if cmd == "dump":
        n = int(argv[argv.index("--verts") + 1]) if "--verts" in argv else 6
        cmd_dump(argv[2], n)
    elif cmd == "stats":
        log = argv[argv.index("--log") + 1] if "--log" in argv else None
        cmd_stats(argv[2], log)
    elif cmd == "seams":
        cmd_seams(argv[2])
    elif cmd == "underside":
        cmd_underside(argv[2])
    elif cmd == "compare":
        tol = float(argv[argv.index("--tol") + 1]) if "--tol" in argv else 0.0
        cmd_compare(argv[2], argv[3], tol)
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
