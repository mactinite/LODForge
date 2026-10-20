"""Validate the recovered .cgid layout against real files.

Usage: python -I cgid_parse.py <grass dir> [detail-file]
"""
import os, struct, sys, collections

def half(h):
    return struct.unpack('<e', struct.pack('<H', h))[0]

def parse(path):
    d = open(path, 'rb').read()
    o = 0
    def take(fmt):
        nonlocal o
        v = struct.unpack_from(fmt, d, o); o += struct.calcsize(fmt); return v
    (ntypes,) = take('<I')
    types = []
    for _ in range(ntypes):
        (nlen,) = take('<I')
        name = d[o:o + nlen]; o += nlen
        assert name.endswith(b'\0'), (path, name)
        wave, formid = take('<fI')
        fit, uni, vlit = take('<BBB')
        (ngroups,) = take('<I')
        groups = []
        for _ in range(ngroups):
            cx, cy, cz, sx, sy, sz, tri, inst, shorts = take('<6f3I')
            data = d[o:o + inst * shorts * 2]; o += inst * shorts * 2
            assert len(data) == inst * shorts * 2, path
            groups.append(((cx, cy, cz), (sx, sy, sz), tri, inst, shorts, data))
        types.append((name[:-1].decode('latin1'), wave, formid, (fit, uni, vlit), groups))
    return types, o, len(d)

def main():
    root = sys.argv[1]
    files = sorted(f for f in os.listdir(root) if f.lower().endswith('.cgid'))
    ok = bad = 0
    shorts_seen = collections.Counter(); ntypes = collections.Counter()
    total_inst = 0; zero_files = 0; errors = []
    for f in files:
        try:
            types, used, size = parse(os.path.join(root, f))
            if used != size:
                raise ValueError(f'trailing {size - used} bytes')
            ok += 1
            ntypes[len(types)] += 1
            if not types: zero_files += 1
            for t in types:
                for g in t[4]:
                    shorts_seen[g[4]] += 1; total_inst += g[3]
        except Exception as e:
            bad += 1; errors.append((f, repr(e)))
    print(f'files={len(files)} ok={ok} bad={bad} empty={zero_files} instances={total_inst}')
    print('shortsPerInstance', dict(shorts_seen))
    print('types/file', sorted(ntypes.items())[:12])
    for e in errors[:5]: print('ERR', e)
    if len(sys.argv) > 2:
        types, _, _ = parse(os.path.join(root, sys.argv[2]))
        for name, wave, formid, flags, groups in types:
            n = sum(g[3] for g in groups)
            print(f'{formid:08X} {name} wave={wave} fit/uni/vlit={flags} groups={len(groups)} inst={n}')
            for g in groups[:2]:
                print('   center', [round(v, 1) for v in g[0]], 'size', [round(v, 1) for v in g[1]], 'tri', g[2], 'inst', g[3])
                for i in range(min(3, g[3])):
                    hw = struct.unpack_from('<16H', g[5], i * 32)
                    print('     ', [round(half(h), 3) for h in hw[:14]], hw[14:])

main()
