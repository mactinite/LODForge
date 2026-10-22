// LODForge guided flow. Talks to the Tauri backend (src/main.rs), which runs the lodforge CLI.
// Outside Tauri (plain browser preview) a small mock backend stands in.
if (!window.__TAURI__) {
  const handlers = {};
  let polls = 0;
  const fire = (name, payload) => (handlers[name] || []).forEach((h) => h({ payload }));
  window.__TAURI__ = {
    core: {
      invoke: async (cmd) => {
        // Preview: ?instance=...&profile=... stands in for the command-line arguments.
        if (cmd === "launch_args") { const q = new URLSearchParams(location.search); return { instance: q.get("instance"), profile: q.get("profile") }; }
        if (cmd === "list_profiles") return ["Default", "Testing"];
        if (cmd === "profile_info") return { plugins: 74, worlds: ["Tamriel", "MarkarthWorld", "Blackreach", "DLC2SolstheimWorld"], seasons: ["SPR", "SUM", "AUT", "WIN"], missing_archives: 0, ms: 212 };
        if (cmd === "mo2_running") return false;
        if (cmd === "start_build") {
          polls = 0;
          // Two lanes as in the real build: terrain on its own thread beside the other stages
          // (their begin/stage events interleave), then the stages that wait for both.
          const lanes = [
            [["Tamriel", "terrain-meshes", null, 3600], ["Tamriel", "terrain-textures", null, 17500], ["Tamriel", "terrain-textures", "WIN", 4200]],
            [["all", "texgen", null, 20000], ["Tamriel", "dyndolod-export", null, 3300], ["Tamriel", "objects", null, 8800]],
          ];
          lanes.forEach((lane, l) => lane.forEach(([world, stage, season, ms], i) => {
            setTimeout(() => fire("build-line", JSON.stringify({ event: "begin", world, stage, season })), 700 * i + 50 + 150 * l);
            setTimeout(() => fire("build-line", JSON.stringify({ event: "stage", world, stage, season, ms })), 700 * (i + 1) + 150 * l);
          }));
          [["all", "plugins", null, 10], ["all", "occlusion", null, 10000]].forEach(([world, stage, season, ms], i) => {
            setTimeout(() => fire("build-line", JSON.stringify({ event: "begin", world, stage, season })), 2500 + 600 * i);
            setTimeout(() => fire("build-line", JSON.stringify({ event: "stage", world, stage, season, ms })), 2500 + 600 * (i + 1));
          });
          return null;
        }
        if (cmd === "quit") { document.body.innerHTML = "<p style='padding:24px'>(mock) LODForge closed.</p>"; return null; }
        if (cmd === "open_viewer") return null;
        if (cmd === "build_status") return ++polls > 8 ? 0 : null;
        return null;
      },
    },
    event: { listen: async (name, h) => { (handlers[name] ||= []).push(h); } },
  };
}
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const state = { step: 0, info: null, worlds: [], seasons: [], expected: 0, done: 0, building: false, running: new Map(), failed: [] };

function store(key, value) { try { localStorage.setItem("lodforge." + key, value); } catch (_) {} }
function load(key, fallback) { try { return localStorage.getItem("lodforge." + key) ?? fallback; } catch (_) { return fallback; } }

function status(text, error = false) { $("status").textContent = text; $("status").className = error ? "error" : ""; }

function show(step) {
  state.step = step;
  document.querySelectorAll(".page").forEach((p) => p.classList.toggle("hidden", Number(p.dataset.page) !== step));
  document.querySelectorAll("#steps li").forEach((li) => {
    const s = Number(li.dataset.step);
    li.classList.toggle("active", s === step);
    li.classList.toggle("done", s < step);
  });
  $("back").disabled = step === 0 || state.building;
  $("next").classList.toggle("hidden", step >= 4);
  $("next").textContent = step === 3 ? "Start build" : "Next";
  $("next").disabled = step === 0 && !state.info;
  if (step === 3) review();
}

async function loadProfiles(preferred) {
  const instance = $("instance").value.trim();
  $("profile").innerHTML = "";
  if (!instance) return;
  try {
    const profiles = await invoke("list_profiles", { instance });
    for (const p of profiles) $("profile").add(new Option(p, p));
    const last = typeof preferred === "string" && profiles.includes(preferred) ? preferred : load("profile", "Default");
    if (profiles.includes(last)) $("profile").value = last;
    if (typeof preferred === "string" && last !== preferred) { status(`Profile "${preferred}" not found in this instance`, true); return false; }
    status(`${profiles.length} profile(s) found`);
    return profiles.length > 0;
  } catch (e) { status(String(e), true); return false; }
}

async function scan() {
  const instance = $("instance").value.trim();
  const profile = $("profile").value;
  if (!instance || !profile) return status("Choose an instance and profile first", true);
  store("instance", instance); store("profile", profile);
  $("scan").disabled = true;
  status("Reading load order and archives…");
  try {
    const info = await invoke("profile_info", { instance, profile });
    state.info = info;
    $("scanResult").classList.remove("hidden");
    $("scanResult").innerHTML = `<b>${info.plugins}</b> plugins &middot; <b>${info.worlds.length}</b> LOD worldspaces &middot; seasons: ${info.seasons.length ? info.seasons.map((s) => `<span class="chip">${s}</span>`).join("") : "none"}` +
      (info.missing_archives ? `<br><span class="hint">${info.missing_archives} archive(s) listed by plugins were not found.</span>` : "") +
      `<br><span class="hint">Scanned in ${info.ms} ms.</span>`;
    renderWorlds(); renderSeasons();
    status("Ready");
  } catch (e) { status(String(e), true); state.info = null; }
  $("scan").disabled = false;
  show(0);
}

function renderWorlds() {
  const box = $("worlds"); box.innerHTML = "";
  for (const w of state.info.worlds) {
    const id = "w-" + w;
    box.insertAdjacentHTML("beforeend", `<label class="check card"><input type="checkbox" id="${id}" data-world="${w}" checked><span>${w}</span></label>`);
  }
}

function renderSeasons() {
  const box = $("seasons"); box.innerHTML = "";
  const names = { SPR: "Spring", SUM: "Summer", AUT: "Autumn", WIN: "Winter" };
  for (const s of state.info.seasons) box.insertAdjacentHTML("beforeend", `<label class="check"><input type="checkbox" data-season="${s}"> <span>${names[s] || s}</span></label>`);
  $("seasonHint").textContent = state.info.seasons.length ? "The default season is always built; tick the extra seasons to generate." : "No season swap files found, so only the default season can be built.";
}

// Quality: a level sets every setting; changing a setting by hand shows "Custom".
const ATLAS_TILES = [256, 512, 1024, 2048];
const LEVELS = {
  performance: { preset: "Low", bc7: "fast", terrainFilter: "hq", atlasTile: 1, billboardScale: 75 },
  balanced: { preset: "Medium", bc7: "high", terrainFilter: "hq", atlasTile: 2, billboardScale: 100 },
  quality: { preset: "High", bc7: "high", terrainFilter: "hq", atlasTile: 3, billboardScale: 150 },
};
const LEVEL_NAMES = { performance: "Performance", balanced: "Balanced", quality: "Quality", custom: "Custom" };
const QUALITY_KEYS = Object.keys(LEVELS.balanced);
const EXPLAIN = {
  preset: {
    Low: "DynDOLOD's Low rules: only larger objects get LOD. Small rocks, cliff trims, roads, fences, walls and clutter are left out; trees are flat billboards everywhere.",
    Medium: "DynDOLOD's Medium rules: most objects get LOD. Hay, road chunks, snow drifts, small rocks and rock piles are left out; trees are billboards.",
    High: "DynDOLOD's High rules: every object with a LOD model gets LOD, small rocks and road chunks included. Trees near you (LOD4) are 3D models, billboards further out. The most draw calls and VRAM.",
  },
  bc7: {
    fast: "BC7 mode 6 on the GPU: seconds per worldspace, but smooth gradients and foliage break into visible 4x4 blocks (terrain normal maps show a block grid).",
    high: "All BC7 modes on the CPU for normal maps, billboards and atlases; terrain colour stays on the GPU because it shows no blocking. Block edges on billboards all but disappear and fine foliage detail survives; on terrain normal maps they roughly halve. Adds about 15 s to Tamriel's terrain textures and 13 s to the billboards.",
    best: "Thorough search for every texture, terrain colour included. Measured no gain over High quality; terrain textures take about 7x and billboards about 2x as long.",
  },
  terrainFilter: {
    hq: "Every LOD level is resized from the whole stitched tile, colour is averaged in linear light and normal maps are renormalised, so distant terrain keeps its brightness (it lost up to 12% before) and its relief.",
    xlodgen: "Reproduces xLODGen: each cell is resized on its own and mips are 8-bit box filters in gamma space. Only useful for comparing against xLODGen output.",
  },
};

function qualityValues() {
  return { preset: $("preset").value, bc7: $("bc7").value, terrainFilter: $("terrainFilter").value, atlasTile: Number($("atlasTile").value), billboardScale: Number($("billboardScale").value) };
}

function syncQualityText() {
  for (const k of ["preset", "bc7", "terrainFilter"]) $(k + "Explain").textContent = EXPLAIN[k][$(k).value] || "";
  const tile = ATLAS_TILES[Number($("atlasTile").value)];
  $("atlasTileOut").textContent = `${tile} px (full textures ${Math.max(64, tile / 4)} px)`;
  const k = Number($("billboardScale").value);
  $("billboardScaleOut").textContent = `${k}% (${(11 / (k / 100)).toFixed(1)} units/px, up to ${Math.min(4096, Math.max(256, Math.round(1024 * k / 100)))} px)`;
}

function currentLevel() {
  const v = qualityValues();
  return Object.keys(LEVELS).find((name) => QUALITY_KEYS.every((key) => LEVELS[name][key] === v[key])) || "custom";
}

function setLevel(name) {
  const r = document.querySelector(`input[name=level][value="${name}"]`);
  if (r) r.checked = true;
}

function applyLevel(name) {
  const l = LEVELS[name];
  if (!l) return;
  for (const key of QUALITY_KEYS) $(key).value = String(l[key]);
  syncQualityText();
  storeQuality();
}

function storeQuality() { store("quality", JSON.stringify(qualityValues())); }

function initQuality() {
  let saved = null;
  try { saved = JSON.parse(load("quality", "null")); } catch (_) {}
  const values = saved && typeof saved === "object" ? { ...LEVELS.balanced, ...saved } : LEVELS.balanced;
  for (const key of QUALITY_KEYS) $(key).value = String(values[key]);
  syncQualityText();
  setLevel(currentLevel());
  document.querySelectorAll("input[name=level]").forEach((r) => r.addEventListener("change", () => { if (r.value !== "custom") applyLevel(r.value); }));
  for (const key of QUALITY_KEYS) $(key).addEventListener("input", () => { syncQualityText(); setLevel(currentLevel()); storeQuality(); });
}

function selection() {
  const worlds = [...document.querySelectorAll("[data-world]:checked")].map((e) => e.dataset.world);
  const seasons = [...document.querySelectorAll("[data-season]:checked")].map((e) => e.dataset.season);
  const dest = document.querySelector("input[name=dest]:checked").value;
  return {
    instance: $("instance").value.trim(), profile: $("profile").value, worlds, seasons,
    terrain: $("terrain").checked, objects: $("objects").checked, trees: $("trees").checked, texgen: $("texgen").checked,
    mode: "dyndolod",
    materials: document.querySelector("input[name=materials]:checked").value,
    preset: $("preset").value, plugins: $("plugins").checked, occlusion: $("occlusion").checked,
    bc7: $("bc7").value, terrain_filter: $("terrainFilter").value,
    billboard_scale: Number($("billboardScale").value) / 100, atlas_max_tile: ATLAS_TILES[Number($("atlasTile").value)],
    grass: $("grass").value, grass_source: $("grassSource").value, grass_density: Number($("grassDensity").value) || null,
    mo2_mod: dest === "mo2" ? $("modName").value.trim() : null,
    out: dest === "folder" ? $("outFolder").value.trim() : null,
    enable: dest === "mo2" && $("enable").checked,
  };
}

function review() {
  const s = selection();
  const grass = { go: "Grass Optimizations meshes", bto: "grass in object LOD" }[s.grass];
  const materials = s.materials === "pbr" ? "PBR (Community Shaders)" : "standard (non-PBR)";
  const stages = [s.texgen && "billboards", s.terrain && "terrain", s.objects && `objects (DynDOLOD ${s.preset})`, s.trees && "trees",
    grass, s.objects && s.plugins && "dynamic LOD plugins", s.objects && s.occlusion && "occlusion"].filter(Boolean);
  const bc7 = { fast: "fast BC7", high: "high-quality BC7", best: "best BC7" }[s.bc7];
  const quality = `${LEVEL_NAMES[currentLevel()]} (${s.preset} object detail, ${bc7}, atlas tiles ${s.atlas_max_tile} px, billboards ${Math.round(s.billboard_scale * 100)}%${s.terrain_filter === "xlodgen" ? ", xLODGen terrain resampling" : ""})`;
  $("review").innerHTML = `<b>Review</b><br>${s.worlds.length} worldspace(s): ${s.worlds.join(", ") || "none"}<br>Stages: ${stages.join(", ") || "none"}<br>Quality: ${quality}<br>Materials: ${materials}<br>Seasons: default${s.seasons.length ? " + " + s.seasons.join(", ") : ""}<br>Output: ${s.mo2_mod ? `MO2 mod "${s.mo2_mod}"${s.enable ? " (enabled)" : ""}` : s.out || "(choose a folder)"}`;
}

async function startBuild() {
  const req = selection();
  if (!req.worlds.length) return status("Select at least one worldspace", true);
  if (!req.terrain && !req.objects && !req.trees && req.grass !== "go") return status("Select at least one stage", true);
  if (!req.mo2_mod && !req.out) return status("Choose where the output goes", true);
  if (req.enable && await invoke("mo2_running")) return status("Close Mod Organizer before enabling the mod, or untick “Enable”", true);
  // DynDOLOD object LOD per worldspace: export, atlas, (grass,) objects.
  const objects = req.objects ? 3 + (req.grass === "bto" ? 1 : 0) : 0;
  const perWorld = (req.terrain ? 1 + 1 + req.seasons.length + 1 : 0) + objects + (req.trees ? 1 : 0);
  // Once per build: TexGen (billboards + stitched textures), DynDOLOD configuration, plugins, occlusion, Grass Optimizations meshes.
  const once = (req.texgen ? 2 : 0) + (req.objects ? 1 + (req.plugins ? 1 : 0) + (req.occlusion ? 1 : 0) : 0) + (req.grass === "go" ? 1 : 0);
  state.expected = perWorld * req.worlds.length + once; state.done = 0; state.running = new Map(); state.failed = [];
  $("stages").tBodies[0].innerHTML = ""; $("log").textContent = ""; $("bar").style.width = "0";
  $("buildTitle").textContent = "Building…"; $("buildSummary").textContent = "";
  try {
    await invoke("start_build", { req });
  } catch (e) { return status(String(e), true); }
  state.building = true; state.started = Date.now(); state.lastRequest = req;
  show(4);
  $("cancel").classList.remove("hidden");
  status("Build running");
  poll();
}

const stageKey = (ev) => `${ev.world}|${ev.stage}|${ev.season ?? ""}`;
const stageLabel = (ev) => `${ev.world === "all" ? "" : ev.world + " "}${ev.stage}${ev.season ? " [" + ev.season + "]" : ""}`;

// A stage that began but never reported its time by the end of the build did not finish (its
// error is in the log). Stages run on two threads (terrain beside the rest), so several are
// running at once and a new one beginning says nothing about the others.
function settleRunning() {
  for (const [key, tr] of state.running) {
    tr.className = "failed";
    tr.lastElementChild.textContent = "failed";
    state.failed.push(key.replace(/\|/g, " ").trim());
  }
  state.running.clear();
}

function showRunning() {
  const labels = [...state.running.values()].map((tr) => tr.dataset.label);
  if (labels.length) status(`Working on ${labels.join(", ")}…`);
}

function beginStage(ev) {
  const tr = document.createElement("tr");
  tr.className = "running";
  tr.dataset.label = stageLabel(ev);
  tr.innerHTML = `<td>${ev.world}</td><td>${ev.stage}</td><td>${ev.season ?? ""}</td><td class="num"><span class="spinner"></span></td>`;
  $("stages").tBodies[0].appendChild(tr);
  state.running.set(stageKey(ev), tr);
  showRunning();
  tr.scrollIntoView({ block: "nearest" });
}

function addStage(ev) {
  state.done += 1;
  $("bar").style.width = Math.min(100, (100 * state.done) / Math.max(1, state.expected)) + "%";
  const time = `${(ev.ms / 1000).toFixed(1)} s`;
  const running = state.running.get(stageKey(ev));
  if (running) {
    state.running.delete(stageKey(ev));
    running.className = "";
    running.lastElementChild.textContent = time;
    showRunning();
    return;
  }
  const tr = document.createElement("tr");
  tr.innerHTML = `<td>${ev.world}</td><td>${ev.stage}</td><td>${ev.season ?? ""}</td><td class="num">${time}</td>`;
  $("stages").tBodies[0].appendChild(tr);
}

async function poll() {
  const code = await invoke("build_status");
  if (code === null) { setTimeout(poll, 500); return; }
  state.building = false;
  settleRunning();
  $("cancel").classList.add("hidden");
  const secs = ((Date.now() - state.started) / 1000).toFixed(1);
  const mins = secs >= 90 ? ` (${(secs / 60).toFixed(1)} min)` : "";
  $("bar").style.width = "100%";
  const ok = code === 0 && state.failed.length === 0;
  const summary = `${state.done} stage(s) in ${secs} s${mins}` + (ok ? "." : code === 0 ? "; some stages did not finish." : ` (exit code ${code}).`);
  $("buildTitle").textContent = ok ? "Done" : "Finished with errors";
  $("buildSummary").textContent = summary;
  status(ok ? "Build finished" : "Build failed", !ok);
  $("back").disabled = false;
  // Done screen.
  const s = state.lastRequest || {};
  $("doneTitle").textContent = ok ? "Build complete" : "Build finished with errors";
  $("doneSummary").textContent = summary;
  $("doneDetail").innerHTML = `<b>Output</b>: ${s.mo2_mod ? `MO2 mod "${s.mo2_mod}"${s.enable ? ", enabled in the profile" : " (enable it in MO2)"}` : s.out || ""}` +
    `<br><b>Worldspaces</b>: ${(s.worlds || []).join(", ")}` +
    (state.failed.length ? `<br><b>Did not finish</b>:<ul>${state.failed.map((f) => `<li>${f}</li>`).join("")}</ul>` : "") +
    (!ok ? `<br><span class="hint">Details are in the log on the Build step.</span>` : "");
  const sel = $("inspectWorld");
  sel.innerHTML = (s.worlds || []).map((w) => `<option>${w}</option>`).join("");
  $("inspect").disabled = !(s.worlds || []).length;
  show(5);
}

listen("build-line", (e) => {
  const line = e.payload;
  $("log").textContent += line + "\n";
  if (line.startsWith("{")) {
    try { const ev = JSON.parse(line); if (ev.event === "stage") addStage(ev); else if (ev.event === "begin") beginStage(ev); } catch (_) {}
  }
});
listen("build-error", (e) => { $("log").textContent += e.payload + "\n"; });

$("instance").value = load("instance", "");
$("instance").addEventListener("change", loadProfiles);
$("scan").addEventListener("click", scan);
$("allWorlds").addEventListener("click", () => document.querySelectorAll("[data-world]").forEach((c) => (c.checked = true)));
$("noWorlds").addEventListener("click", () => document.querySelectorAll("[data-world]").forEach((c) => (c.checked = false)));
$("back").addEventListener("click", () => show(Math.max(0, state.step - 1)));
$("next").addEventListener("click", () => (state.step === 3 ? startBuild() : show(state.step + 1)));
$("cancel").addEventListener("click", async () => { await invoke("cancel_build"); status("Cancelled", true); });
$("closeApp").addEventListener("click", () => invoke("quit"));
$("inspect").addEventListener("click", async () => {
  const s = state.lastRequest || {};
  const world = $("inspectWorld").value;
  if (!world) return status("No worldspace to inspect", true);
  try {
    await invoke("open_viewer", { req: { instance: s.instance, profile: s.profile, world, out: s.out || null, mo2_mod: s.mo2_mod || null } });
    status(`Viewer opened on ${world}`);
  } catch (e) { status(String(e), true); }
});
$("again").addEventListener("click", () => show(2));
document.querySelectorAll("input[name=dest], #modName, #outFolder, #enable").forEach((el) => el.addEventListener("input", review));
function syncGrass() {
  const g = $("grass").value;
  $("grassSourceBox").classList.toggle("hidden", g !== "bto");
  $("grassDensityBox").classList.toggle("hidden", g !== "bto");
  $("grassHint").textContent = g === "bto"
    ? "Grass billboards in the nearest object LOD, like DynDOLOD's grass LOD. Needs an NGIO grass cache (precache in game); cells without a cache file get no grass."
    : "Grass Optimizations (Community Shaders) shows grass far out with NGIO's extended grass distance and swaps in these crossed-plane meshes; no grass cache is needed.";
  store("grass", g);
}
$("grass").addEventListener("change", syncGrass);
initQuality();
$("grass").value = load("grass", "off");
if (!$("grass").value) $("grass").value = "off";
syncGrass();
try { localStorage.removeItem("lodforge.mode"); } catch (_) {}
{
  const saved = load("materials", "pbr");
  const r = document.querySelector(`input[name=materials][value="${saved === "complex" ? "standard" : saved}"]`);
  if (r) r.checked = true;
  document.querySelectorAll("input[name=materials]").forEach((el) => el.addEventListener("change", () => store("materials", el.value)));
}
// `lodforge-ui --instance <folder> [--profile <name>]` (e.g. from the MO2 tool entry) wins over
// the remembered instance and scans straight away.
(async () => {
  show(0);
  let args = {};
  try { args = (await invoke("launch_args")) || {}; } catch (_) {}
  if (args.instance || args.profile) {
    if (args.instance) $("instance").value = args.instance;
    if (await loadProfiles(args.profile || undefined)) await scan();
  } else {
    loadProfiles();
  }
})();
