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
          [["Tamriel", "terrain-meshes", null, 3600], ["Tamriel", "terrain-textures", null, 17500], ["Tamriel", "dyndolod-export", null, 3300], ["Tamriel", "objects", null, 8800], ["all", "plugins", null, 10], ["all", "occlusion", null, 10000]].forEach(([world, stage, season, ms], i) =>
            setTimeout(() => fire("build-line", JSON.stringify({ event: "stage", world, stage, season, ms })), 300 * (i + 1)));
          return null;
        }
        if (cmd === "build_status") return ++polls > 4 ? 0 : null;
        return null;
      },
    },
    event: { listen: async (name, h) => { (handlers[name] ||= []).push(h); } },
  };
}
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const state = { step: 0, info: null, worlds: [], seasons: [], expected: 0, done: 0, building: false };

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
  $("next").classList.toggle("hidden", step === 4);
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
    box.insertAdjacentHTML("beforeend", `<label class="check card"><input type="checkbox" id="${id}" data-world="${w}" ${w === "Tamriel" ? "checked" : ""}><span>${w}</span></label>`);
  }
}

function renderSeasons() {
  const box = $("seasons"); box.innerHTML = "";
  const names = { SPR: "Spring", SUM: "Summer", AUT: "Autumn", WIN: "Winter" };
  for (const s of state.info.seasons) box.insertAdjacentHTML("beforeend", `<label class="check"><input type="checkbox" data-season="${s}"> <span>${names[s] || s}</span></label>`);
  $("seasonHint").textContent = state.info.seasons.length ? "The default season is always built; tick the extra seasons to generate." : "No season swap files found, so only the default season can be built.";
}

function selection() {
  const worlds = [...document.querySelectorAll("[data-world]:checked")].map((e) => e.dataset.world);
  const seasons = [...document.querySelectorAll("[data-season]:checked")].map((e) => e.dataset.season);
  const dest = document.querySelector("input[name=dest]:checked").value;
  return {
    instance: $("instance").value.trim(), profile: $("profile").value, worlds, seasons,
    terrain: $("terrain").checked, objects: $("objects").checked, trees: $("trees").checked, texgen: $("texgen").checked,
    mode: "dyndolod",
    preset: $("preset").value, plugins: $("plugins").checked, occlusion: $("occlusion").checked,
    grass: $("grass").value, grass_source: $("grassSource").value, grass_density: Number($("grassDensity").value) || null,
    mo2_mod: dest === "mo2" ? $("modName").value.trim() : null,
    out: dest === "folder" ? $("outFolder").value.trim() : null,
    enable: dest === "mo2" && $("enable").checked,
  };
}

function review() {
  const s = selection();
  const grass = { go: "Grass Optimizations meshes", bto: "grass in object LOD" }[s.grass];
  const stages = [s.texgen && "billboards", s.terrain && "terrain", s.objects && `objects (DynDOLOD ${s.preset})`, s.trees && "trees",
    grass, s.objects && s.plugins && "dynamic LOD plugins", s.objects && s.occlusion && "occlusion"].filter(Boolean);
  $("review").innerHTML = `<b>Review</b><br>${s.worlds.length} worldspace(s): ${s.worlds.join(", ") || "none"}<br>Stages: ${stages.join(", ") || "none"}<br>Seasons: default${s.seasons.length ? " + " + s.seasons.join(", ") : ""}<br>Output: ${s.mo2_mod ? `MO2 mod "${s.mo2_mod}"${s.enable ? " (enabled)" : ""}` : s.out || "(choose a folder)"}`;
}

async function startBuild() {
  const req = selection();
  if (!req.worlds.length) return status("Select at least one worldspace", true);
  if (!req.terrain && !req.objects && !req.trees && req.grass !== "go") return status("Select at least one stage", true);
  if (!req.mo2_mod && !req.out) return status("Choose where the output goes", true);
  if (req.enable && await invoke("mo2_running")) return status("Close Mod Organizer before enabling the mod, or untick “Enable”", true);
  // DynDOLOD object LOD per worldspace: export, atlas, (grass,) objects.
  const objects = req.objects ? 3 + (req.grass === "bto" ? 1 : 0) : 0;
  const perWorld = (req.terrain ? 1 + 1 + req.seasons.length : 0) + objects + (req.trees ? 1 : 0);
  // Once per build: TexGen, DynDOLOD configuration, plugins, occlusion, Grass Optimizations meshes.
  const once = (req.texgen ? 1 : 0) + (req.objects ? 1 + (req.plugins ? 1 : 0) + (req.occlusion ? 1 : 0) : 0) + (req.grass === "go" ? 1 : 0);
  state.expected = perWorld * req.worlds.length + once; state.done = 0;
  $("stages").tBodies[0].innerHTML = ""; $("log").textContent = ""; $("bar").style.width = "0";
  $("buildTitle").textContent = "Building…"; $("buildSummary").textContent = "";
  try {
    await invoke("start_build", { req });
  } catch (e) { return status(String(e), true); }
  state.building = true; state.started = Date.now();
  show(4);
  $("cancel").classList.remove("hidden");
  status("Build running");
  poll();
}

function addStage(ev, failed = false) {
  state.done += 1;
  $("bar").style.width = Math.min(100, (100 * state.done) / Math.max(1, state.expected)) + "%";
  const tr = document.createElement("tr");
  if (failed) tr.className = "failed";
  tr.innerHTML = `<td>${ev.world}</td><td>${ev.stage}</td><td>${ev.season ?? ""}</td><td class="num">${(ev.ms / 1000).toFixed(1)} s</td>`;
  $("stages").tBodies[0].appendChild(tr);
}

async function poll() {
  const code = await invoke("build_status");
  if (code === null) { setTimeout(poll, 500); return; }
  state.building = false;
  $("cancel").classList.add("hidden");
  const secs = ((Date.now() - state.started) / 1000).toFixed(1);
  $("bar").style.width = "100%";
  $("buildTitle").textContent = code === 0 ? "Done" : "Finished with errors";
  $("buildSummary").textContent = `${state.done} stage(s) in ${secs} s` + (code === 0 ? "." : ` (exit code ${code}); see the log.`);
  status(code === 0 ? "Build finished" : "Build failed", code !== 0);
  $("back").disabled = false;
}

listen("build-line", (e) => {
  const line = e.payload;
  $("log").textContent += line + "\n";
  if (line.startsWith("{")) {
    try { const ev = JSON.parse(line); if (ev.event === "stage") addStage(ev); } catch (_) {}
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
$("grass").value = load("grass", "off");
if (!$("grass").value) $("grass").value = "off";
syncGrass();
try { localStorage.removeItem("lodforge.mode"); } catch (_) {}
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
