import { useCallback, useEffect, useRef, useState } from "react";

import { BuildEvent, invoke, listen } from "./backend";
import { Values, toArgs } from "./options";
import { LogLine, classify } from "./stages";

/** A value kept in localStorage under `lodforge.<key>` (ignored when storage is unavailable). */
export function useStored<T>(key: string, initial: T): [T, (v: T | ((p: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const s = localStorage.getItem("lodforge." + key);
      return s === null ? initial : (JSON.parse(s) as T);
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("lodforge." + key, JSON.stringify(value));
    } catch {
      /* private window or blocked storage */
    }
  }, [key, value]);
  return [value, setValue];
}

export interface Output {
  dest: "mo2" | "folder";
  modName: string;
  folder: string;
  enable: boolean;
}

export interface Target {
  instance: string;
  profile: string;
  worlds: string[];
  seasons: string[];
}

export interface StageRow {
  key: string;
  world: string;
  stage: string;
  season: string;
  ms: number | null;
  state: "running" | "done" | "failed";
  /** ms since the build started, for the timeline. */
  startedAt: number;
}

export interface BuildState {
  running: boolean;
  stages: StageRow[];
  log: LogLine[];
  /** Lines that report a failed stage or a crash. */
  errors: number;
  warnings: number;
  exitCode: number | null;
  startedAt: number;
  finishedAt: number | null;
}

const empty: BuildState = { running: false, stages: [], log: [], errors: 0, warnings: 0, exitCode: null, startedAt: 0, finishedAt: null };

/** The command line a build runs (shown in the UI and sent to the backend). */
export function buildArgs(t: Target, v: Values, o: Output): string[] {
  const args = ["build", "--json", "--instance", t.instance, "--profile", t.profile, "--mode", "dyndolod"];
  if (t.worlds.length) args.push("--worlds", t.worlds.join(","));
  if (t.seasons.length) args.push("--seasons", t.seasons.join(","));
  args.push(...toArgs(v));
  if (o.dest === "mo2") {
    args.push("--mo2-mod", o.modName);
    if (o.enable) args.push("--enable");
  } else {
    args.push("--out", o.folder);
  }
  return args;
}

export function useBuild() {
  const [state, setState] = useState<BuildState>(empty);
  const started = useRef(0);

  useEffect(() => {
    const subs = [
      listen("build-line", (p) => {
        const line = String(p);
        let ev: BuildEvent | null = null;
        if (line.startsWith("{")) {
          try {
            ev = JSON.parse(line) as BuildEvent;
          } catch {
            /* not JSON */
          }
        }
        setState((s) => {
          const entry = classify(line, false);
          const log = [...s.log, entry];
          const counted = { errors: s.errors + (entry.level === "error" ? 1 : 0), warnings: s.warnings + (entry.level === "warning" ? 1 : 0) };
          if (!ev || !ev.stage) return { ...s, ...counted, log };
          const key = `${ev.world}|${ev.stage}|${ev.season ?? ""}`;
          const stages = [...s.stages];
          const i = stages.findIndex((r) => r.key === key && r.state === "running");
          if (ev.event === "begin") {
            stages.push({ key, world: ev.world ?? "", stage: ev.stage, season: ev.season ?? "", ms: null, state: "running", startedAt: Date.now() - started.current });
          } else if (ev.event === "stage") {
            if (i >= 0) stages[i] = { ...stages[i], ms: ev.ms ?? 0, state: "done" };
            else stages.push({ key, world: ev.world ?? "", stage: ev.stage, season: ev.season ?? "", ms: ev.ms ?? 0, state: "done", startedAt: Date.now() - started.current - (ev.ms ?? 0) });
          }
          return { ...s, ...counted, log, stages };
        });
      }),
      listen("build-error", (p) =>
        setState((s) => {
          const entry = classify(String(p), true);
          // A stage failure marks that stage's running row (the CLI sends no event for it).
          const f = entry.failed;
          const stages = f ? s.stages.map((r) => (r.state === "running" && r.stage === f.stage && (r.world === f.world || f.world === "all") ? { ...r, state: "failed" as const } : r)) : s.stages;
          return { ...s, stages, log: [...s.log, entry], errors: s.errors + (entry.level === "error" ? 1 : 0), warnings: s.warnings + (entry.level === "warning" ? 1 : 0) };
        }),
      ),
    ];
    return () => {
      subs.forEach((u) => u.then((f) => f()));
    };
  }, []);

  const start = useCallback(async (args: string[]) => {
    started.current = Date.now();
    setState({ ...empty, running: true, startedAt: started.current });
    await invoke("start_build", { args });
    const poll = async () => {
      const code = await invoke<number | null>("build_status");
      if (code === null) {
        setTimeout(poll, 500);
        return;
      }
      setState((s) => ({
        ...s,
        running: false,
        exitCode: code,
        finishedAt: Date.now(),
        stages: s.stages.map((r) => (r.state === "running" ? { ...r, state: "failed" } : r)),
      }));
    };
    poll();
  }, []);

  const cancel = useCallback(() => invoke("cancel_build"), []);
  return { state, start, cancel };
}
