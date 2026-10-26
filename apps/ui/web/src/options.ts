// Every `lodforge build` option the UI exposes, with its flag, default and explanation. The
// Advanced view renders from this list, the Guided view sets a subset through quality levels, and
// toArgs() turns the values into the CLI argument list (the backend checks each flag against its
// own allow-list). Inventory: docs/ui-options-inventory.md.

export type OptionValue = string | number | boolean;

export interface Choice {
  value: string;
  label: string;
  explain: string;
}

interface Base {
  key: string;
  flag: string;
  label: string;
  explain: string;
  group: Group;
  /** Shown only when this returns true for the current values. */
  when?: (v: Values) => boolean;
  /** Expert options start collapsed in the Advanced view. */
  expert?: boolean;
}

export type Option =
  | (Base & { kind: "bool"; default: boolean; /** flag means "off" (e.g. --no-plugins) */ negate?: boolean })
  | (Base & { kind: "choice"; default: string; choices: Choice[] })
  | (Base & { kind: "number"; default: number; min: number; max: number; step: number; unit?: string; format?: (n: number) => string })
  | (Base & { kind: "text"; default: string; placeholder?: string });

export type Values = Record<string, OptionValue>;

export type Group = "stages" | "runtime" | "objects" | "trees" | "terrain" | "textures" | "grass" | "plugins" | "input";

export const GROUPS: { id: Group; label: string; description: string }[] = [
  { id: "stages", label: "Stages", description: "What the build generates." },
  { id: "runtime", label: "LODForge Runtime", description: "Instance files for the LODForge Runtime SKSE plugin: octahedral tree impostors, instanced object LOD, far grass and dynamic LOD." },
  { id: "objects", label: "Objects (DynDOLOD)", description: "Object LOD rules, child worldspaces and LOD models." },
  { id: "trees", label: "Trees", description: "Billboards, Ultra tree LOD and generated 3D tree models." },
  { id: "terrain", label: "Terrain", description: "Terrain LOD meshes and textures." },
  { id: "textures", label: "Textures and atlas", description: "Compression, atlas tile sizes and texture tone." },
  { id: "grass", label: "Grass LOD", description: "Distant grass." },
  { id: "plugins", label: "Plugins and occlusion", description: "DynDOLOD.esm/.esp, Occlusion.esp and their update behaviour." },
  { id: "input", label: "Input", description: "Which mods and plugins the build reads." },
];

const isRuntime = (v: Values) => v.runtime === "lodforge";
const isUltra = (v: Values) => v.treeLod === "ultra" || isRuntime(v);
const hasModels = (v: Values) => isUltra(v) && v.treeModels !== "off";

export const OPTIONS: Option[] = [
  // Stages
  { key: "terrain", flag: "--no-terrain", negate: true, kind: "bool", default: true, group: "stages", label: "Terrain LOD", explain: "Meshes plus diffuse and normal textures for every LOD level." },
  { key: "objects", flag: "--no-objects", negate: true, kind: "bool", default: true, group: "stages", label: "Object LOD", explain: "Distant buildings, rocks, mountains and (Ultra) trees with the texture atlas." },
  { key: "trees", flag: "--no-trees", negate: true, kind: "bool", default: true, group: "stages", label: "Tree LOD", explain: "The game's billboard tree LOD (Classic), or the empty tree lists Ultra needs." },
  { key: "texgen", flag: "--texgen", kind: "bool", default: true, group: "stages", label: "Render billboards (TexGen)", explain: "Render tree and trunk billboards and the stitched LOD textures from the load order first, so no TexGen output has to be installed." },
  // LODForge Runtime
  { key: "runtime", flag: "--runtime", kind: "choice", default: "none", group: "runtime", label: "Runtime", explain: "Who draws distant trees and repeated objects.", choices: [
    { value: "none", label: "Game + DynDOLOD DLL", explain: "Classic output: everything merged into the game's .bto/.btt files." },
    { value: "lodforge", label: "LODForge Runtime", explain: "Trees become octahedral impostors (3D LOD4 models up close), repeated objects and large references are instanced, far grass from the grass cache. Needs the LODForge Runtime SKSE plugin." },
  ] },
  { key: "impostorUnitsPerPx", flag: "--impostor-units-per-px", kind: "number", default: 12, min: 6, max: 24, step: 2, group: "runtime", when: isRuntime, label: "Impostor resolution", explain: "Game units per impostor pixel; frames are sized by tree height (smaller = sharper, more VRAM).", format: (n) => `${n} units/px` },
  { key: "impostorMaxFrame", flag: "--impostor-max-frame", kind: "choice", default: "128", group: "runtime", when: isRuntime, expert: true, label: "Largest impostor frame", explain: "Upper limit of the per-view frame size (large trees use the large-tree frame).", choices: [
    { value: "64", label: "64 px", explain: "Least VRAM." },
    { value: "128", label: "128 px", explain: "Default." },
    { value: "256", label: "256 px", explain: "Sharpest; pages fill up four times faster." },
  ] },
  { key: "impostorLargeFrame", flag: "--impostor-large-frame", kind: "choice", default: "256", group: "runtime", when: isRuntime, expert: true, label: "Large-tree impostor frame", explain: "Frame size of trees taller than 2600 units (giant oaks, Aspens on Riften hills).", choices: [
    { value: "0", label: "No large tier", explain: "Large trees use the normal frame limit." },
    { value: "256", label: "256 px", explain: "Default." },
    { value: "512", label: "512 px", explain: "Very sharp; large VRAM cost." },
  ] },
  { key: "impostorAlpha", flag: "--impostor-alpha", kind: "choice", default: "coverage", group: "runtime", when: isRuntime, expert: true, label: "Impostor alpha", explain: "How crown coverage is stored in the impostor pages.", choices: [
    { value: "coverage", label: "Coverage", explain: "Alpha-tested coverage kept through the mips (default)." },
    { value: "binary", label: "Binary", explain: "Opaque crown interiors; for Community Shaders Snow Cover's sky-exposure test." },
  ] },
  { key: "runtimeGrass", flag: "--runtime-no-grass", negate: true, kind: "bool", default: true, group: "runtime", when: isRuntime, label: "Far grass", explain: "Far grass (.lfg) from the NGIO grass cache. Skipped automatically when the load order has no Grass\\*.cgid cache." },
  { key: "runtimeGrassCache", flag: "--runtime-grass-cache", kind: "text", default: "", group: "runtime", when: (v) => isRuntime(v) && v.runtimeGrass !== false, expert: true, label: "Grass cache folder", placeholder: "Grass\\ of the load order", explain: "Folder with the NGIO grass cache (.cgid) when it is not in the load order." },
  { key: "runtimeLargeRefs", flag: "--runtime-no-large-refs", negate: true, kind: "bool", default: true, group: "runtime", when: isRuntime, expert: true, label: "Instanced large references", explain: "The runtime draws and hands off every eligible large reference (no large-reference grid bugs)." },
  { key: "runtimeMinCopies", flag: "--runtime-min-copies", kind: "number", default: 8, min: 0, max: 64, step: 1, group: "runtime", when: isRuntime, expert: true, label: "Instancing threshold", explain: "LOD models with at least this many copies per LOD level are instanced instead of merged (0 = off).", format: (n) => (n ? `${n} copies` : "off") },
  { key: "runtimeNoDyndolodPlugins", flag: "--runtime-no-dyndolod-plugins", kind: "bool", default: false, group: "runtime", when: isRuntime, expert: true, label: "No DynDOLOD plugins", explain: "Skip DynDOLOD.esm/.esp: the runtime draws dynamic LOD from the .dyn.bin files. Occlusion.esp is still written." },
  // Objects
  { key: "preset", flag: "--preset", kind: "choice", default: "Medium", group: "objects", label: "Object LOD detail", explain: "DynDOLOD's rule preset.", choices: [
    { value: "Low", label: "Low", explain: "Only larger objects get LOD; small rocks, trims, roads, fences and clutter are left out." },
    { value: "Medium", label: "Medium", explain: "Most objects get LOD; hay, road chunks, snow drifts and small rocks are left out." },
    { value: "High", label: "High", explain: "Every object with a LOD model gets LOD. Ultra: 3D trees at LOD4. Most draw calls and VRAM." },
  ] },
  { key: "childParent", flag: "--child-parent", kind: "choice", default: "Low", group: "objects", label: "Child > parent copies", explain: "Which IgnoreChildEDID list filters what walled cities copy into Tamriel's LOD (DynDOLOD's own setting, separate from the preset).", choices: [
    { value: "Low", label: "Low", explain: "DynDOLOD's default: the longest ignore list, fewest copies." },
    { value: "Medium", label: "Medium", explain: "More city objects show from outside." },
    { value: "High", label: "High", explain: "The shortest ignore list: most city objects get distant copies." },
  ] },
  { key: "lodModels", flag: "--lod-models", kind: "bool", default: true, group: "objects", label: "Generate missing LOD models", explain: "Make LOD models from full models where the rules want LOD and none is installed (meshes\\lod\\lodforge\\)." },
  { key: "lodModelMinSize", flag: "--lod-model-min-size", kind: "text", default: "", group: "objects", when: (v) => Boolean(v.lodModels), expert: true, label: "Generated model minimum size", placeholder: "256,512,1024", explain: "Smallest object size (game units) that gets a generated LOD4, LOD8, LOD16 model. Empty keeps 256, 512, 1024." },
  { key: "keepFarLodModels", flag: "--keep-far-lod-models", kind: "bool", default: false, group: "objects", expert: true, label: "Keep shipped LOD8/16 models", explain: "By default LOD8/16 models whose texture differs from LOD4 are rebuilt from the LOD4 model." },
  { key: "windows", flag: "--windows", kind: "bool", default: false, group: "objects", label: "Glowing windows", explain: "LOD window overlays that glow at night." },
  { key: "staticLod", flag: "--static-lod", kind: "bool", default: false, group: "objects", expert: true, label: "Static LOD only", explain: "Static rules only; no dynamic LOD (no DynDOLOD.esm/.esp)." },
  { key: "largeRefWorkarounds", flag: "--no-large-ref-workarounds", negate: true, kind: "bool", default: true, group: "objects", expert: true, label: "Large reference workarounds", explain: "Markers, RNAM entries and reverts that hide the engine's large-reference bugs." },
  // Trees
  { key: "treeLod", flag: "--tree-lod", kind: "choice", default: "classic", group: "trees", when: (v) => !isRuntime(v), label: "Tree LOD", explain: "Where distant trees come from.", choices: [
    { value: "classic", label: "Classic", explain: "Billboards in the game's tree LOD (at most 256 tree types per worldspace)." },
    { value: "ultra", label: "Ultra", explain: "Every tree in object LOD: 3D models where the preset asks for them (High: LOD4), billboards elsewhere." },
  ] },
  { key: "treeModels", flag: "--tree-models", kind: "choice", default: "off", group: "trees", when: isUltra, label: "Generated 3D tree models", explain: "Make 3D LOD models for trees big enough for them (billboard-cloud crown over a reduced 3D trunk).", choices: [
    { value: "off", label: "Off", explain: "Installed 3D tree LOD models only; the rest stay billboards." },
    { value: "generate", label: "Generate", explain: "Generate models for trees without an installed CRC-matching 3D model." },
    { value: "all", label: "Generate all", explain: "Generate for every tree, replacing installed 3D models too." },
  ] },
  { key: "treeModelCrown", flag: "--tree-model-crown", kind: "choice", default: "cloud", group: "trees", when: hasModels, label: "Crown method", explain: "How the leaves of a generated model are made.", choices: [
    { value: "cloud", label: "Billboard cloud", explain: "About 15 textured planes keep the crown's outline and density at ~30 triangles." },
    { value: "prune", label: "Pruned cards", explain: "TreeLODGen's method: fewer of the original leaf cards. Heavy (thousands of triangles per tree)." },
  ] },
  { key: "treeModelLevels", flag: "--tree-model-levels", kind: "text", default: "", group: "trees", when: hasModels, expert: true, label: "Model level budgets", placeholder: "0.4:0.8:400:2500,0.15:0.7:150:1000,0.06:0.6:60:400", explain: "keep:area:bark:floor[:max_scale] for LOD4, LOD8, LOD16. Bark is the 3D trunk's triangle budget." },
  { key: "billboardScale", flag: "--billboard-scale", kind: "number", default: 1, min: 0.5, max: 2, step: 0.25, group: "trees", label: "Billboard resolution", explain: "Pixels per tree billboard; the tree atlas grows with its square.", format: (n) => `${Math.round(n * 100)}% (${(11 / n).toFixed(1)} units/px)` },
  { key: "billboardSupersample", flag: "--billboard-supersample", kind: "number", default: 4, min: 1, max: 8, step: 1, group: "trees", expert: true, label: "Billboard supersampling", explain: "Each billboard is rendered this many times larger per axis and averaged down (1 = TexGen's hard alpha cut).", format: (n) => `${n}x` },
  { key: "billboardDensity", flag: "--billboard-density", kind: "number", default: 1, min: 0.1, max: 1, step: 0.05, group: "trees", expert: true, label: "Billboard density", explain: "Opaque share of tree billboards relative to their rendered coverage.", format: (n) => `${Math.round(n * 100)}%` },
  { key: "billboardSoft", flag: "--billboard-soft", kind: "bool", default: false, group: "trees", expert: true, label: "Soft billboard alpha", explain: "Keep the rescaled soft alpha instead of dithering it (dithering keeps thin foliage through the game's alpha test)." },
  { key: "installedBillboards", flag: "--installed-billboards", kind: "choice", default: "render", group: "trees", expert: true, label: "Mod-installed billboards", explain: "Billboards a mod ships ready-made for trees TexGen's rule would skip.", choices: [
    { value: "render", label: "Render them too", explain: "Every tree gets the same supersampled quality." },
    { value: "keep", label: "Keep installed", explain: "Use the installed billboards, as TexGen does." },
  ] },
  // Terrain
  { key: "terrainFilter", flag: "--terrain-filter", kind: "choice", default: "hq", group: "terrain", label: "Terrain texture resampling", explain: "How terrain LOD textures are resized.", choices: [
    { value: "hq", label: "Smooth", explain: "Whole stitched tiles, linear-light averaging, renormalised normal maps: distant terrain keeps its brightness and relief." },
    { value: "xlodgen", label: "xLODGen-compatible", explain: "Per cell, 8-bit gamma-space box mips, as xLODGen does. Only for comparisons." },
  ] },
  { key: "lod32", flag: "--lod32", kind: "choice", default: "step", group: "terrain", label: "World map terrain (LOD32)", explain: "The world map's terrain level.", choices: [
    { value: "step", label: "High detail (STEP)", explain: "2048 px textures, full mesh budget. About 130 MB more on Tamriel." },
    { value: "golden", label: "xLODGen default", explain: "512 px textures, mesh quality 10." },
  ] },
  { key: "terrainMips", flag: "--terrain-mips", kind: "choice", default: "full", group: "terrain", expert: true, label: "Terrain texture mips", explain: "Mip chains of the terrain textures.", choices: [
    { value: "full", label: "Full", explain: "Every mip level." },
    { value: "golden", label: "As xLODGen", explain: "The mip count xLODGen writes." },
    { value: "none", label: "None", explain: "Top level only." },
  ] },
  { key: "terrainPbrScale", flag: "--terrain-pbr-scale", kind: "number", default: 0.65, min: 0.65, max: 1, step: 0.05, group: "terrain", expert: true, label: "PBR landscape brightness", explain: "Colour scale for PBR landscape albedo in terrain LOD.", format: (n) => n.toFixed(2) },
  { key: "lodFill", flag: "--no-lod-fill", negate: true, kind: "bool", default: true, group: "terrain", expert: true, label: "Fill cells without LAND", explain: "Fill cells without landscape from the shipped terrain LOD instead of leaving them flat." },
  { key: "underside", flag: "--no-underside", negate: true, kind: "bool", default: true, group: "terrain", expert: true, label: "Terrain underside", explain: "DynDOLOD's terrain underside mesh that hides gaps under the terrain LOD." },
  // Textures
  { key: "materials", flag: "--materials", kind: "choice", default: "pbr", group: "textures", label: "Materials", explain: "Which texture variants LOD uses.", choices: [
    { value: "pbr", label: "PBR (Community Shaders)", explain: "pbr_lod texture twins and PBR texture sets from textures\\pbr\\." },
    { value: "standard", label: "Standard", explain: "Plain textures for non-PBR lists (ENB complex material, parallax or vanilla)." },
  ] },
  { key: "bc7", flag: "--bc7", kind: "choice", default: "high", group: "textures", label: "Texture compression (BC7)", explain: "BC7 encoder for every texture written.", choices: [
    { value: "fast", label: "Fast (GPU)", explain: "Seconds per worldspace, but gradients and foliage show 4x4 blocks." },
    { value: "high", label: "High quality", explain: "All BC7 modes; block edges on billboards all but disappear. Recommended." },
    { value: "best", label: "Best", explain: "Thorough search; measured no gain over High, several times slower." },
  ] },
  { key: "atlasMaxTile", flag: "--atlas-max-tile", kind: "number", default: 1024, min: 256, max: 2048, step: 0, group: "textures", label: "Object texture detail", explain: "Largest size a LOD texture keeps on the object LOD atlas (full textures get a quarter).", format: (n) => `${n} px (full textures ${Math.max(64, n / 4)} px)` },
  { key: "lodTextureTone", flag: "--lod-texture-tone", kind: "choice", default: "flat", group: "textures", label: "Distant texture tone", explain: "Brightness of stitched LOD8/16 textures.", choices: [
    { value: "flat", label: "Match LOD4", explain: "Keep the source brightness so LOD8/16 match LOD4." },
    { value: "dyndolod", label: "DynDOLOD rules", explain: "TexGen's per-texture brightness rules (mountains ~14% darker)." },
  ] },
  { key: "farLodMatch", flag: "--far-lod-match", kind: "number", default: 0.5, min: 0, max: 1, step: 0.05, group: "textures", expert: true, label: "Far LOD texture match", explain: "Moves LOD8+-only pre-baked textures toward the LOD4 texture's brightness.", format: (n) => n.toFixed(2) },
  // Grass
  { key: "grass", flag: "--grass", kind: "choice", default: "off", group: "grass", when: (v) => !isRuntime(v), label: "Grass LOD", explain: "Distant grass.", choices: [
    { value: "off", label: "Off", explain: "No grass LOD." },
    { value: "go", label: "Grass Optimizations meshes", explain: "Crossed-plane grass meshes for Community Shaders' Grass Optimizations; no cache needed." },
    { value: "bto", label: "Grass in object LOD", explain: "Grass billboards in the nearest object LOD, like DynDOLOD. Needs an NGIO grass cache." },
  ] },
  { key: "grassDensity", flag: "--grass-density", kind: "number", default: 50, min: 5, max: 100, step: 5, group: "grass", when: (v) => !isRuntime(v) && v.grass === "bto", label: "Grass density", explain: "Share of the cached grass placements kept in LOD.", format: (n) => `${n}%` },
  // Plugins
  { key: "plugins", flag: "--no-plugins", negate: true, kind: "bool", default: true, group: "plugins", label: "Dynamic LOD plugins", explain: "DynDOLOD.esm, DynDOLOD.esp and the DynDOLOD DLL NG data files." },
  { key: "occlusion", flag: "--no-occlusion", negate: true, kind: "bool", default: true, group: "plugins", label: "Occlusion", explain: "Occlusion.esp from the new terrain and object LOD." },
  { key: "newToken", flag: "--new-token", kind: "bool", default: false, group: "plugins", expert: true, label: "New run token", explain: "Start a new run token even when updating an earlier build in place." },
  { key: "renumber", flag: "--renumber", kind: "bool", default: false, group: "plugins", expert: true, label: "Renumber plugins", explain: "Number DynDOLOD.esm/.esp from scratch. Breaks saves made with the earlier output." },
  // Input
  { key: "excludeMod", flag: "--exclude-mod", kind: "text", default: "", group: "input", label: "Exclude mods", placeholder: "Mod A,Mod B", explain: "Enabled mods to leave out of the input (comma separated). Earlier LOD output is left out automatically." },
  { key: "excludePlugin", flag: "--exclude-plugin", kind: "text", default: "", group: "input", expert: true, label: "Exclude plugins", placeholder: "Some.esp", explain: "Active plugins to leave out of the load order (comma separated)." },
  { key: "keepLodMods", flag: "--keep-lod-mods", kind: "bool", default: false, group: "input", expert: true, label: "Read earlier LOD output", explain: "Also read LOD output mods that are normally left out." },
];

export const DEFAULTS: Values = Object.fromEntries(OPTIONS.map((o) => [o.key, o.default]));

/** Quality levels for the Guided view: each sets these keys. */
export const LEVELS: Record<string, { label: string; description: string; values: Values }> = {
  performance: { label: "Performance", description: "Fewer LOD objects, fast compression, billboard trees. Quickest build, lightest on VRAM.", values: { preset: "Low", bc7: "fast", atlasMaxTile: 512, billboardScale: 0.75, treeLod: "classic", treeModels: "off" } },
  balanced: { label: "Balanced", description: "DynDOLOD's Medium rules, high-quality compression, billboard trees. Recommended.", values: { preset: "Medium", bc7: "high", atlasMaxTile: 1024, billboardScale: 1, treeLod: "classic", treeModels: "off" } },
  quality: { label: "Quality", description: "High rules, Ultra trees with generated 3D models near you, larger atlas tiles.", values: { preset: "High", bc7: "high", atlasMaxTile: 2048, billboardScale: 1.5, treeLod: "ultra", treeModels: "generate" } },
};

export function levelOf(v: Values): string {
  return Object.entries(LEVELS).find(([, l]) => Object.entries(l.values).every(([k, x]) => v[k] === x))?.[0] ?? "custom";
}

export function optionByKey(key: string): Option | undefined {
  return OPTIONS.find((o) => o.key === key);
}

/** The CLI arguments for the options (only those that differ from the CLI's defaults, plus visible ones). */
export function toArgs(v: Values): string[] {
  const args: string[] = [];
  for (const o of OPTIONS) {
    if (o.when && !o.when(v)) continue;
    const x = v[o.key] ?? o.default;
    switch (o.kind) {
      case "bool":
        if (o.negate ? x === false : x === true) args.push(o.flag);
        break;
      case "choice":
        if (x !== o.default || ["preset", "materials", "bc7", "treeLod"].includes(o.key)) args.push(o.flag, String(x));
        break;
      case "number":
        if (x !== o.default || o.key === "atlasMaxTile") {
          args.push(o.flag, String(x));
          if (o.key === "atlasMaxTile") args.push("--atlas-full-max-tile", String(Math.max(64, Number(x) / 4)));
        }
        break;
      case "text":
        if (String(x).trim()) args.push(o.flag, String(x).trim());
        break;
    }
  }
  return args;
}
