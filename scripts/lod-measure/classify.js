// node classify.js dump.json [--json out.json]
// Per category x pass: draws, instances, triangles, replay GPU ms. Categories from Community Shaders'
// frame annotations "[<ShaderType>:<desc>] (<pass>) <hint> <geometry name> -> <shader cache path>".
const fs = require('fs');
const file = process.argv[2];
const jsonOut = process.argv.includes('--json') ? process.argv[process.argv.indexOf('--json') + 1] : null;
const d = JSON.parse(fs.readFileSync(file, 'utf8'));
if (d.error) { console.error(d.error); process.exit(1); }
const re = /^\[(\w+):([0-9A-F]+)\] \(([0-9A-F]+)\) <[^>]*> (.*?) -> (.*)$/;

function category(a) {
  for (let i = a.path.length - 1; i >= 0; i--) {
    const m = re.exec(a.path[i]);
    if (!m) continue;
    const type = m[1], tech = type === 'Lighting' ? ((parseInt(m[2], 16) >>> 24) & 0x3f) : -1;
    const g = m[4].toLowerCase();
    if (type === 'Grass') return 'grass';
    if (type === 'DistantTree') return 'tree LOD billboards (.btt)';
    if (g === 'land' || tech === 9 || tech === 18) return 'terrain LOD';
    if (g === 'treepassthru') return 'object LOD: 3D trees';
    if (g === 'objpassthru') return 'object LOD: passthru';
    if (/^obj(snow)?(hd)?(-largeref)?$/.test(g) || tech === 13 || tech === 15) return 'object LOD: atlas';
    if (tech === 12) return 'full trees';
    if (tech === 8 || tech === 19 || /^block \(/.test(g)) return 'terrain (loaded cells)';
    if (type === 'Sky') return 'sky';
    if (type === 'Water') return 'water';
    if (type === 'Effect') return 'effects';
    return 'other objects';
  }
  return a.dispatch ? 'compute/post' : 'unannotated';
}
function pass(a) {
  const p = a.path[1] || '(none)';
  if (p === 'World') return 'main';
  if (p === 'Depth') return 'depth';
  if (/Shadowmaps/.test(p)) return 'sun shadows';
  if (p === 'Water Effects') return 'water cubemap';
  return 'other';
}

const passNames = ['main', 'depth', 'sun shadows', 'water cubemap', 'other'];
const cats = {};
let totalMs = 0, totalDraws = 0;
for (const a of d.actions) {
  const c = category(a), p = pass(a);
  const tris = a.dispatch ? 0 : Math.floor((a.indices || 0) / 3) * Math.max(1, a.instances || 1);
  const ms = (a.gpu_s || 0) * 1000;
  const r = cats[c] || (cats[c] = { draws: 0, instances: 0, tris: 0, ms: 0, byPass: {} });
  r.draws++; r.instances += Math.max(1, a.instances || 1); r.tris += tris; r.ms += ms;
  const q = r.byPass[p] || (r.byPass[p] = { draws: 0, ms: 0 });
  q.draws++; q.ms += ms;
  totalMs += ms; if (!a.dispatch) totalDraws++;
}
const f = (n, w) => String(n).padStart(w);
console.log(`${file}: ${totalDraws} draws, replay GPU ${totalMs.toFixed(1)} ms`);
console.log('  category                     draws   GPU %   tris(M)   ' + passNames.map(p => p.padStart(14)).join(''));
for (const [c, r] of Object.entries(cats).sort((x, y) => y[1].ms - x[1].ms)) {
  const per = passNames.map(p => f(r.byPass[p] ? `${r.byPass[p].draws}/${(100 * r.byPass[p].ms / totalMs).toFixed(1)}%` : '-', 14)).join('');
  console.log(`  ${c.padEnd(28)}${f(r.draws, 6)}${f((100 * r.ms / totalMs).toFixed(1), 8)}${f((r.tris / 1e6).toFixed(2), 10)}   ${per}`);
}
if (jsonOut) fs.writeFileSync(jsonOut, JSON.stringify({ file, totalDraws, totalMs, cats }, null, 1));
