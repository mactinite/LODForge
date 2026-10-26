// The Tauri backend (apps/ui/src/main.rs) runs the lodforge CLI. In a plain browser (dev preview)
// a mock stands in, so every screen can be developed and checked without Tauri.

export type Unlisten = () => void;

interface TauriGlobal {
  core: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
  event: { listen: (name: string, h: (e: { payload: unknown }) => void) => Promise<Unlisten> };
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
  }
}

export interface ProfileInfo {
  plugins: number;
  worlds: string[];
  seasons: string[];
  missing_archives: number;
  /** SKSE\Plugins\LODForgeRuntime.dll is in the load order (older CLIs leave it out). */
  runtime_plugin?: boolean;
  /** NGIO grass cache files (Grass\*.cgid) in the load order. */
  grass_cache?: number;
  ms: number;
}

/** One config file LODForge reads (see `config_files` in main.rs). */
export interface ConfigFile {
  /** Path below the DynDOLOD folder, e.g. `Configs\DynDOLOD_SSE_mod_world_ignore.txt`. */
  name: string;
  /** Format id, see configFormats.ts. */
  format: string;
  /** Where the file in use comes from: `overlay` (LODForge Config mod), `dyndolod` or `missing`. */
  source: "overlay" | "dyndolod" | "missing";
  size: number;
  /** Read by the LODForge build (the rest are DynDOLOD/TexGen-only). */
  used: boolean;
}

export interface BuildEvent {
  event: string;
  world?: string;
  stage?: string;
  season?: string | null;
  ms?: number;
  [k: string]: unknown;
}

const mock = (() => {
  const handlers: Record<string, ((e: { payload: unknown }) => void)[]> = {};
  const fire = (name: string, payload: unknown) => (handlers[name] || []).forEach((h) => h({ payload }));
  let polls = 0;
  const overlay: Record<string, string> = {};
  const files: Record<string, string> = {
    "Configs\\DynDOLOD_SSE_mod_world_ignore.txt": "// ignore all references added by mod to worldspace\r\nJKs Skyrim.esp=MarkarthWorld,SolitudeWorld,WindhelmWorld\r\n",
    "Configs\\DynDOLOD_SSE_childworldfull.txt": "; references whose copies keep the full model\r\nSkyrim.esm;000E0B6C\r\nSkyrim.esm;000E0B6D\r\n",
    "Configs\\DynDOLOD_SSE_ChildworldMatches.txt": "skyrim.esm;000E73A4;skyrim.esm;000F4FEE;SCastleWall06\r\n",
    "Configs\\DynDOLOD_SSE_mesh_crc32.txt": "// crc32 of meshes\r\nmeshes\\landscape\\trees\\treepineforest01.nif;6F2D14FB\r\n",
    "Rules\\DynDOLOD_SSE_High.ini": "[Skyrim LODGen]\r\nLODGen1=roadchunk,Static LOD4,Static LOD8,Static LOD16,,Unchanged,1\r\nLODGen2=sovngardestatue0,,,,Far LOD,Unchanged,0\r\nLODGen3=\\,Static LOD4,Static LOD8,Static LOD16,Far LOD,Unchanged,1\r\n",
  };
  const formatOf = (n: string) =>
    n.includes("mod_world_ignore") ? "mod_world_ignore" : n.includes("childworldfull") ? "plugin_formid" : n.includes("ChildworldMatches") ? "childworld_matches" : n.includes("mesh_crc32") ? "mesh_crc" : n.startsWith("Rules") ? "rules" : "text";
  const treeReport =
    "plugin,form_id,editor_id,model,model_crc32,references,lod4,lod8,lod16,lod4_model,lod4_source,note,generator_candidate\r\n" +
    "Skyrim.esm,07614B,TreeAspen05,Meshes\\Landscape\\Trees\\srg_oak5.nif,476F9698,1757,3d,billboard,billboard,meshes\\dyndolod\\lod\\trees\\srg_oak5_476f9698passthru_lod.nif,Mod: LODForge Output,,yes\r\n" +
    "Skyrim.esm,05C06E,TreePineForestSnow05,Meshes\\Landscape\\Trees\\TreePineForestSnow05.nif,11223344,1481,3d,billboard,billboard,x,Mod: LODForge Output,,yes\r\n" +
    "Skyrim.esm,0A7329,TreeThicket01,Meshes\\Landscape\\Plants\\Thicket01.nif,,11988,none,none,none,,,no LOD (no billboard),no\r\n" +
    "Skyrim.esm,0ABD65,TreeThicket01Snow,Meshes\\Landscape\\Plants\\Thicket01Snow.nif,,6532,billboard,billboard,billboard,t.dds,,no 3D LOD model installed,no\r\n";
  return {
    invoke: async <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> => {
      const r = (v: unknown) => v as T;
      switch (cmd) {
        case "launch_args": {
          const q = new URLSearchParams(location.search);
          return r({ instance: q.get("instance"), profile: q.get("profile") });
        }
        case "list_profiles":
          return r(["Default", "Testing"]);
        case "profile_info":
          return r({ plugins: 236, worlds: ["Tamriel", "MarkarthWorld", "Blackreach", "DLC2SolstheimWorld"], seasons: ["SPR", "SUM", "AUT", "WIN"], missing_archives: 0, runtime_plugin: true, grass_cache: 0, ms: 212 });
        case "mo2_running":
          return r(false);
        case "config_files":
          return r(Object.keys(files).map((name) => ({ name, format: formatOf(name), source: overlay[name] !== undefined ? "overlay" : "dyndolod", size: (overlay[name] ?? files[name]).length, used: true })));
        case "read_config":
          return r(overlay[args.name as string] ?? files[args.name as string] ?? "");
        case "write_config":
          overlay[args.name as string] = args.text as string;
          return r(null);
        case "reset_config":
          delete overlay[args.name as string];
          return r(null);
        case "read_report":
          return r(treeReport);
        case "start_build": {
          // Same check as main.rs check_build_args.
          const a = (args.args as string[]) ?? [];
          if (a[0] !== "build") throw "the command must start with `build`";
          if (a.some((x, i) => x === "--mode" && a[i + 1] !== "dyndolod")) throw "the UI builds DynDOLOD style only (--mode dyndolod)";
          polls = 0;
          const stages: [string, string, string | null, number][] = [
            ["Tamriel", "terrain-meshes", null, 3600], ["all", "texgen", null, 20000], ["all", "tree-models", null, 105000],
            ["Tamriel", "terrain-textures", null, 17500], ["Tamriel", "dyndolod-export", null, 3300], ["Tamriel", "objects", null, 131000],
            ["all", "plugins", null, 7900], ["all", "occlusion", null, 517000],
          ];
          stages.forEach(([world, stage, season, ms], i) => {
            setTimeout(() => fire("build-line", JSON.stringify({ event: "begin", world, stage, season })), 600 * i + 50);
            setTimeout(() => fire("build-line", JSON.stringify({ event: "stage", world, stage, season, ms })), 600 * (i + 1));
          });
          return r(null);
        }
        case "build_status":
          return r(++polls > 12 ? 0 : null);
        default:
          return r(null);
      }
    },
    listen: async (name: string, h: (e: { payload: unknown }) => void): Promise<Unlisten> => {
      (handlers[name] ||= []).push(h);
      return () => {
        handlers[name] = (handlers[name] || []).filter((x) => x !== h);
      };
    },
  };
})();

export const isTauri = !!window.__TAURI__;

export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI__ ? window.__TAURI__.core.invoke<T>(cmd, args) : mock.invoke<T>(cmd, args);
}

export function listen(name: string, h: (payload: unknown) => void): Promise<Unlisten> {
  const wrap = (e: { payload: unknown }) => h(e.payload);
  return window.__TAURI__ ? window.__TAURI__.event.listen(name, wrap) : mock.listen(name, wrap);
}
