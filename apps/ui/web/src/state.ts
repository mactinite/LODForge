import { useCallback, useEffect, useRef, useState } from "react";

import { BuildEvent, invoke, listen } from "./backend";
import { Values, toArgs } from "./options";

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
  log: string[];
  errors: string[];
  exitCode: number | null;
  startedAt: number;
  finishedAt: number | null;
}

const empty: BuildState = { running: false, stages: [], log: [], errors: [], exitCode: null, startedAt: 0, finishedAt: null };

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
          const log = [...s.log, line];
          if (!ev || !ev.stage) return { ...s, log };
          const key = `${ev.world}|${ev.stage}|${ev.season ?? ""}`;
          const stages = [...s.stages];
          const i = stages.findIndex((r) => r.key === key && r.state === "running");
          if (ev.event === "begin") {
            stages.push({ key, world: ev.world ?? "", stage: ev.stage, season: ev.season ?? "", ms: null, state: "running", startedAt: Date.now() - started.current });
          } else if (ev.event === "stage") {
            if (i >= 0) stages[i] = { ...stages[i], ms: ev.ms ?? 0, state: "done" };
            else stages.push({ key, world: ev.world ?? "", stage: ev.stage, season: ev.season ?? "", ms: ev.ms ?? 0, state: "done", startedAt: Date.now() - started.current - (ev.ms ?? 0) });
          }
          return { ...s, log, stages };
        });
      }),
      listen("build-error", (p) => setState((s) => ({ ...s, log: [...s.log, String(p)], errors: [...s.errors, String(p)] }))),
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
