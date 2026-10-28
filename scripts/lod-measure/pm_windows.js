// node pm_windows.js <presentmon.csv> <phases.txt> [hours to add to the CSV clock, e.g. 7]
// PresentMon 2.x CSV (recorded with --date_time) sliced into the windows in phases.txt
// ("<label> HH:mm:ss.fff HH:mm:ss.fff", local time). Prints frames, FPS and the median / mean / p95
// of FrameTime, CPUBusy, CPUWait, GPUBusy and GPUTime per window, plus on-vs-off deltas per spot.
const fs = require('fs');
const [csvPath, phasesPath, shiftArg] = process.argv.slice(2);
const shift = shiftArg ? parseFloat(shiftArg) * 3600 : 0; // hours added to the CSV clock to match phases.txt

const lines = fs.readFileSync(csvPath, 'utf8').split(/\r?\n/).filter(Boolean);
const head = lines[0].replace(/^﻿/, '').split(',');
const col = name => head.indexOf(name);
const alias = { CPUStartTime: 'CPUStartDateTime', FrameTime: 'MsBetweenAppStart', CPUBusy: 'MsCPUBusy', CPUWait: 'MsCPUWait', GPUBusy: 'MsGPUBusy', GPUTime: 'MsGPUTime' };
const need = Object.keys(alias);
for (const [k, v] of Object.entries(alias)) if (head.indexOf(k) < 0 && head.indexOf(v) >= 0) head[head.indexOf(v)] = k;
for (const n of need) if (col(n) < 0) { console.error(`missing column ${n}; columns: ${head.join(',')}`); process.exit(1); }

const tod = s => { // seconds since midnight from "... HH:MM:SS.fffffff"
  const m = /(\d{1,2}):(\d{2}):(\d{2}(?:\.\d+)?)/.exec(s);
  return m ? (+m[1]) * 3600 + (+m[2]) * 60 + parseFloat(m[3]) : NaN;
};
const rows = lines.slice(1).map(l => l.split(',')).map(c => ({
  t: (tod(c[col('CPUStartTime')]) + shift + 86400) % 86400,
  ...Object.fromEntries(need.slice(1).map(n => [n, parseFloat(c[col(n)])])),
}));

const stat = (xs) => {
  const v = xs.filter(Number.isFinite).sort((a, b) => a - b);
  if (!v.length) return { med: NaN, mean: NaN, p95: NaN };
  return { med: v[Math.floor(v.length / 2)], mean: v.reduce((a, b) => a + b, 0) / v.length, p95: v[Math.floor(v.length * 0.95)] };
};

const phases = fs.readFileSync(phasesPath, 'utf8').split(/\r?\n/).filter(Boolean).map(l => {
  const [label, s, e] = l.trim().split(/\s+/);
  return { label, s: tod(s), e: tod(e) };
});

const out = {};
const f = x => (Number.isFinite(x) ? x.toFixed(2) : '-').padStart(7);
console.log('window   frames    fps |  frame med/mean/p95   |   CPUBusy med/mean   |  CPUWait med  |  GPUBusy med/mean   | GPUTime med');
for (const p of phases) {
  const w = rows.filter(r => r.t >= p.s && r.t <= p.e);
  const st = Object.fromEntries(need.slice(1).map(n => [n, stat(w.map(r => r[n]))]));
  const fps = w.length / (p.e - p.s);
  out[p.label] = { frames: w.length, fps, ...st };
  console.log(`${p.label.padEnd(7)}${String(w.length).padStart(7)}${f(fps)} |${f(st.FrameTime.med)}${f(st.FrameTime.mean)}${f(st.FrameTime.p95)} |${f(st.CPUBusy.med)}${f(st.CPUBusy.mean)}       |${f(st.CPUWait.med)}       |${f(st.GPUBusy.med)}${f(st.GPUBusy.mean)}      |${f(st.GPUTime.med)}`);
}
console.log('\nspot  frame med on->off (delta)   CPUBusy med on->off (delta)   GPUBusy med on->off (delta)');
for (const spot of [...new Set(phases.map(p => p.label.split('-')[0]))]) {
  const on = out[`${spot}-on`], off = out[`${spot}-off`];
  if (!on || !off) continue;
  const d = (k) => `${on[k].med.toFixed(2)} -> ${off[k].med.toFixed(2)} (${(on[k].med - off[k].med).toFixed(2)} ms, ${(100 * (on[k].med - off[k].med) / on[k].med).toFixed(0)}%)`;
  console.log(`${spot.padEnd(5)} ${d('FrameTime').padEnd(30)} ${d('CPUBusy').padEnd(30)} ${d('GPUBusy')}`);
}
