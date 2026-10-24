// Table formats of the flat config files LODForge reads (docs/ui-options-inventory.md). Each file
// parses into lines: data rows (cells) or kept-as-is lines (comments, blanks, INI sections), so a
// save writes the file back in its own layout with only the edited rows changed.

export interface Column {
  label: string;
  width?: number;
  placeholder?: string;
  /** Allowed values (a select instead of a text field). */
  options?: string[];
  /** Short help shown in the column header tooltip. */
  help?: string;
}

export interface Format {
  id: string;
  label: string;
  description: string;
  columns: Column[];
  /** Splits a data line into cells; null = keep the line as is (comment, blank, section). */
  parse: (line: string, section: string) => string[] | null;
  join: (cells: string[], section: string) => string;
  /** INI files: data rows belong to a section. */
  sections?: boolean;
}

/** `orig` is the line as read: a row whose cells are unchanged is written back exactly as it was. */
export type Line = { kind: "row"; cells: string[]; section: string; orig?: { text: string; cells: string[] } } | { kind: "raw"; text: string; section: string };

const isComment = (l: string) => {
  const t = l.trim();
  return t === "" || t.startsWith("//") || t.startsWith(";") || t.startsWith("#");
};

const LEVELS = ["", "Static LOD4", "Static LOD8", "Static LOD16", "Static LOD32", "Level0", "Level1", "Level2", "Level3", "Full model", "Billboard1", "Billboard2", "Billboard3", "Billboard4", "Billboard5", "Billboard6"];
const GRIDS = ["", "Near LOD", "Far LOD", "NeverFade LOD", "Near Full", "Far Full", "NeverFade Full"];

export const FORMATS: Record<string, Format> = {
  mod_world_ignore: {
    id: "mod_world_ignore",
    label: "Mod / worldspace ignore",
    description: "References a plugin adds to (or overrides in) these worldspaces get no LOD and are not copied into the parent worldspace.",
    columns: [{ label: "Plugin", placeholder: "JKs Skyrim.esp" }, { label: "Worldspaces", placeholder: "MarkarthWorld,SolitudeWorld", help: "Comma-separated worldspace EditorIDs" }],
    parse: (l) => (isComment(l) || !l.includes("=") ? null : [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]),
    join: (c) => `${c[0]}=${c[1]}`,
  },
  plugin_formid: {
    id: "plugin_formid",
    label: "Plugin and form ID",
    description: "A list of references: plugin file and the reference's form ID (hex, without the load order index).",
    columns: [{ label: "Plugin", placeholder: "Skyrim.esm" }, { label: "Form ID", width: 140, placeholder: "000E0B6C" }, { label: "Rest", help: "Anything after the form ID, kept as written" }],
    parse: (l) => {
      if (isComment(l) || !l.includes(";")) return null;
      const p = l.split(";");
      return [p[0].trim(), (p[1] ?? "").trim(), p.slice(2).join(";")];
    },
    join: (c) => [c[0], c[1], ...(c[2] ? [c[2]] : [])].join(";"),
  },
  childworld_matches: {
    id: "childworld_matches",
    label: "Child world matches",
    description: "Known pairs of a child worldspace reference and the parent reference that already stands for it: the child reference is not copied.",
    columns: [{ label: "Child plugin" }, { label: "Child ID", width: 110 }, { label: "Parent plugin" }, { label: "Parent ID", width: 110 }, { label: "Note" }],
    parse: (l) => {
      if (isComment(l) || l.split(";").length < 4) return null;
      const p = l.split(";");
      return [p[0], p[1], p[2], p[3], p.slice(4).join(";")].map((s) => s.trim());
    },
    join: (c) => c.slice(0, 4).concat(c[4] ? [c[4]] : []).join(";"),
  },
  mesh_crc: {
    id: "mesh_crc",
    label: "Mesh CRC32 table",
    description: "The CRC32 of vanilla full meshes. A mesh at such a path with another CRC was replaced by a mod: only LOD models named <stem>_<crc> fit it.",
    columns: [{ label: "Mesh", placeholder: "meshes\\landscape\\trees\\treepineforest01.nif" }, { label: "CRC32", width: 120, placeholder: "6F2D14FB" }],
    parse: (l) => (isComment(l) || !l.includes(";") ? null : [l.slice(0, l.lastIndexOf(";")).trim(), l.slice(l.lastIndexOf(";") + 1).trim()]),
    join: (c) => `${c[0]};${c[1]}`,
  },
  path_list: {
    id: "path_list",
    label: "Path list",
    description: "One Data-relative path per line.",
    columns: [{ label: "Path" }],
    parse: (l) => (isComment(l) ? null : [l.trim()]),
    join: (c) => c[0],
  },
  mesh_replace: {
    id: "mesh_replace",
    label: "Mesh replacements",
    description: "A mesh path and the mesh used in its place (paths below meshes\).",
    columns: [{ label: "Mesh" }, { label: "Replaced by" }],
    parse: (l) => (isComment(l) || !l.includes("=") ? null : [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]),
    join: (c) => `${c[0]}=${c[1]}`,
  },
  texture_copy: {
    id: "texture_copy",
    label: "TexGen texture copies",
    description: "TexGen copies the first texture to the second path.",
    columns: [{ label: "Source texture" }, { label: "Copy to" }],
    parse: (l) => (isComment(l) || !l.includes(",") ? null : [l.slice(0, l.indexOf(",")).trim(), l.slice(l.indexOf(",") + 1).trim()]),
    join: (c) => `${c[0]},${c[1]}`,
  },
  texgen_billboards: {
    id: "texgen_billboards",
    label: "TexGen billboards",
    description: "Trees TexGen must (True) or must not (False) make a billboard for, optionally with a scale.",
    columns: [{ label: "Plugin" }, { label: "Form ID", width: 110 }, { label: "EditorID" }, { label: "Billboard", width: 110, options: ["", "True", "False"] }, { label: "Scale", width: 90 }],
    parse: (l) => {
      if (isComment(l) || !l.includes(";")) return null;
      const [head, ...flags] = l.split(",");
      const p = head.split(";");
      const scale = flags.find((f) => f.trim().toLowerCase().startsWith("scale="))?.split("=")[1] ?? "";
      const flag = flags.find((f) => ["true", "false"].includes(f.trim().toLowerCase()))?.trim() ?? "";
      return [p[0].trim(), (p[1] ?? "").trim(), (p[2] ?? "").trim(), flag, scale.trim()];
    },
    join: (c) => [c[0], c[1], ...(c[2] ? [c[2]] : [])].join(";") + (c[3] ? `,${c[3]}` : "") + (c[4] ? `,Scale=${c[4]}` : ""),
  },
  rules: {
    id: "rules",
    label: "LOD rules",
    description: "First matching rule wins. Mask: part of the model path below meshes\\ (\\ = everything), or plugin;FORMID for one reference. Levels say what LOD4/8/16/32 show; Grid is the dynamic LOD range.",
    sections: true,
    columns: [
      { label: "Key", width: 90 },
      { label: "Mask", placeholder: "landscape\\trees\\" },
      { label: "LOD4", width: 130, options: LEVELS },
      { label: "LOD8", width: 130, options: LEVELS },
      { label: "LOD16", width: 130, options: LEVELS },
      { label: "LOD32", width: 130, options: LEVELS },
      { label: "Grid", width: 130, options: GRIDS },
      { label: "Reference", width: 120, options: ["Unchanged", "Replace", "Disable"] },
      { label: "Flags", width: 70, help: "1 VWD, 2 no glow, 4 no MATO, 8 dynamic, 16 tree, 32 map" },
    ],
    parse: (l) => {
      if (isComment(l) || l.trim().startsWith("[") || !l.includes("=")) return null;
      const key = l.slice(0, l.indexOf("=")).trim();
      const f = l.slice(l.indexOf("=") + 1).split(",");
      // Other settings in rule files (IgnoreWorlds=..., IgnoreParentWorlds=...) are kept as is.
      if (f.length < 7) return null;
      // 7 fields: no LOD32 column.
      const cells = f.length >= 8 ? [f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]] : [f[0], f[1], f[2], f[3], "", f[4] ?? "", f[5] ?? "", f[6] ?? ""];
      return [key, ...cells.map((s) => s.trim())];
    },
    join: (c) => {
      const [key, mask, l4, l8, l16, l32, grid, refa, flags] = c;
      const levels = l32 ? [l4, l8, l16, l32] : [l4, l8, l16];
      return `${key}=${[mask, ...levels, grid, refa, flags || "0"].join(",")}`;
    },
  },
  ini: {
    id: "ini",
    label: "INI settings",
    description: "key=value settings by section.",
    sections: true,
    columns: [{ label: "Key", width: 260 }, { label: "Value" }],
    parse: (l) => (isComment(l) || l.trim().startsWith("[") || !l.includes("=") ? null : [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]),
    join: (c) => `${c[0]}=${c[1]}`,
  },
};

export function parseFile(text: string, format: Format): Line[] {
  let section = "";
  return text.split(/\r?\n/).map((raw) => {
    const t = raw.trim();
    if (t.startsWith("[") && t.endsWith("]")) {
      section = t.slice(1, -1);
      return { kind: "raw", text: raw, section };
    }
    const cells = format.parse(raw, section);
    return cells ? { kind: "row", cells, section, orig: { text: raw, cells: [...cells] } } : { kind: "raw", text: raw, section };
  });
}

export function joinFile(lines: Line[], format: Format): string {
  // Drop a trailing empty line the split produced; files end with CRLF.
  const same = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
  const out = lines.map((l) => (l.kind === "raw" ? l.text : l.orig && same(l.cells, l.orig.cells) ? l.orig.text : format.join(l.cells, l.section)));
  while (out.length && out[out.length - 1] === "") out.pop();
  return out.join("\r\n") + "\r\n";
}

/** Rule level parsed to a number for charts: 0 none, 1 3D, 2 billboard, 3 full. */
export function levelKind(v: string): "none" | "3d" | "billboard" | "full" {
  const s = v.toLowerCase();
  if (!s) return "none";
  if (s.startsWith("billboard")) return "billboard";
  if (s.startsWith("full")) return "full";
  return "3d";
}
