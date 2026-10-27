// Stage ids of `lodforge build --json` and how the build log's lines are graded.

/** Plain names for the CLI's stage ids; runtime stages are marked so a runtime build's extra work is visible. */
export const STAGE_LABELS: Record<string, { label: string; runtime?: boolean }> = {
  texgen: { label: "Billboards (TexGen)" },
  "texgen-stitched": { label: "Stitched LOD textures" },
  "dyndolod-config": { label: "Rules and LOD models" },
  "lod-models": { label: "Generate missing LOD models" },
  "tree-models": { label: "Generate 3D tree models" },
  "terrain-meshes": { label: "Terrain meshes" },
  "terrain-textures": { label: "Terrain textures" },
  "dyndolod-export": { label: "Object export" },
  "far-lod-models": { label: "Far LOD models" },
  impostors: { label: "Octahedral impostors", runtime: true },
  "runtime-trees": { label: "Runtime trees (impostors + 3D near tier)", runtime: true },
  "runtime-grass": { label: "Far grass", runtime: true },
  "runtime-dyn": { label: "Dynamic LOD data", runtime: true },
  "tree-pack": { label: "Instanced mesh packs", runtime: true },
  "dyndolod-atlas": { label: "Object LOD atlas" },
  "objects-atlas": { label: "Object LOD atlas" },
  "pbr-linear": { label: "PBR linear textures" },
  objects: { label: "Object LOD meshes" },
  trees: { label: "Tree LOD" },
  grass: { label: "Grass LOD" },
  "grass-lod1": { label: "Grass LOD" },
  underside: { label: "Terrain underside" },
  plugins: { label: "DynDOLOD plugins" },
  occlusion: { label: "Occlusion" },
};

export const stageLabel = (stage: string) => STAGE_LABELS[stage]?.label ?? stage;

export type LogLevel = "error" | "warning" | "info";

export interface LogLine {
  text: string;
  level: LogLevel;
  /** For a stage failure: the stage it reports (world "all" for build-wide stages). */
  failed?: { world: string; stage: string };
}

// Error-stream lines that are only information (the CLI writes notes there too).
const INFO = [/^note:/i, /\+Border dropped$/, /^occlusion timing /, /^profile .* loose files/, /^LOD distances /];
// Error-stream lines worth a look that do not fail a stage.
const WARN = [/^warning:/i, /^occlusion lane:/, /^stale output lookup:/, /^\s*no (HD )?billboard for /, /^\s*no plugin for /, /^skipping:/];

/**
 * Grades one line of build output. `stderr`: it came on the error stream. A stage failure is
 * `<stage>: <error>` or `<world> <stage>: <error>` (build.rs, terrain_lane.rs); a crash is a
 * Rust panic or anyhow's final `Error: ...`.
 */
export function classify(text: string, stderr: boolean): LogLine {
  const t = text.trimEnd();
  if (/panicked at|^Error: /.test(t)) return { text, level: "error" };
  if (!stderr) {
    if (/^\s*warning:/i.test(t)) return { text, level: "warning" };
    return { text, level: "info" };
  }
  if (INFO.some((r) => r.test(t))) return { text, level: "info" };
  const m = /^(?:(\S+) )?([a-z0-9-]+): /.exec(t);
  if (m && STAGE_LABELS[m[2]]) return { text, level: "error", failed: { world: m[1] ?? "all", stage: m[2] } };
  if (WARN.some((r) => r.test(t))) return { text, level: "warning" };
  // Anything else on the error stream: shown as a warning, never counted as a failure.
  return { text, level: "warning" };
}
