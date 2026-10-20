"""Check group caps and the recovered group-bounds formula over a grass dir."""
import os, sys, struct, collections

here = os.path.dirname(os.path.abspath(__file__))
exec(open(os.path.join(here, 'cgid_parse.py')).read().split('def main')[0])

root = sys.argv[1]
mx = 0; gpt = collections.Counter(); ok = bad = 0; prefixes = collections.Counter()
for f in os.listdir(root):
    types, _, _ = parse(os.path.join(root, f))
    cx = int(f.split('x')[1].split('y')[0]); cy = int(f.split('y')[1].split('.')[0])
    ox = 12 * int(cx / 12) * 4096.0; oy = 12 * int(cy / 12) * 4096.0
    for name, wave, fid, flags, groups in types:
        gpt[len(groups)] += 1
        prefixes[name.split(chr(92))[0]] += 1
        for c, s, tri, inst, shorts, data in groups:
            mx = max(mx, inst)
            pts = [struct.unpack_from('<3H', data, i * 32) for i in range(inst)]
            xs = [half(p[0]) + ox for p in pts]; ys = [half(p[1]) + oy for p in pts]; zs = [half(p[2]) for p in pts]
            lo = (min(xs) - 30, min(ys) - 30, min(zs) - 30); hi = (max(xs) + 30, max(ys) + 30, max(zs) + 30)
            cc = tuple(l + (h - l) * 0.5 for l, h in zip(lo, hi)); ss = tuple(h - m for h, m in zip(hi, cc))
            if all(abs(a - b) < 0.01 for a, b in zip(cc + ss, c + s)):
                ok += 1
            else:
                bad += 1
                if bad < 4: print('mismatch', f, name, c, s, cc, ss)
print('max instances/group', mx, 'groups/type', sorted(gpt.items())[:8], 'bounds ok/bad', ok, bad)
print('model prefixes', prefixes.most_common(5))
