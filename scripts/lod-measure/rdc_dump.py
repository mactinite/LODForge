# Run inside qrenderdoc:  set RDC_IN=<capture.rdc> RDC_OUT=<out.json> ; qrenderdoc.exe --python rdc_dump.py
# Dumps every action (draw / dispatch / marker) with its marker path, index/instance counts and GPU time.
import os, json, traceback
import renderdoc as rd

out_path = os.environ.get("RDC_OUT", "rdc_dump.json")
log = []
prog = open(out_path + ".progress", "w")
def P(m):
    prog.write(m + "\n"); prog.flush()

def main():
    path = os.environ["RDC_IN"]
    P("open " + path)
    cap = rd.OpenCaptureFile()
    status = cap.OpenFile(path, "", None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("OpenFile failed: %s" % status)
    status, ctrl = cap.OpenCapture(rd.ReplayOptions(), None)
    if status != rd.ResultCode.Succeeded:
        raise RuntimeError("OpenCapture failed: %s" % status)
    P("replay ready")
    sf = ctrl.GetStructuredFile()

    times = {}
    try:
      if os.environ.get("RDC_COUNTERS", "1") == "1":
        P("counters start")
        counters = ctrl.EnumerateCounters()
        if rd.GPUCounter.EventGPUDuration in counters:
            for r in ctrl.FetchCounters([rd.GPUCounter.EventGPUDuration]):
                times[r.eventId] = r.value.d
        P("counters done %d" % len(times))
    except Exception as e:
        log.append("counters failed: %s" % e)

    rows = []
    def walk(actions, stack):
        for a in actions:
            name = a.GetName(sf)
            flags = int(a.flags)
            is_draw = bool(a.flags & rd.ActionFlags.Drawcall)
            is_disp = bool(a.flags & rd.ActionFlags.Dispatch)
            if is_draw or is_disp:
                rows.append({
                    "eid": a.eventId,
                    "name": name,
                    "path": stack,
                    "indices": a.numIndices,
                    "instances": a.numInstances,
                    "indirect": bool(a.flags & rd.ActionFlags.Indirect),
                    "dispatch": is_disp,
                    "gpu_s": times.get(a.eventId),
                })
            if len(a.children):
                walk(a.children, stack + [name])
    walk(ctrl.GetRootActions(), [])

    total = sum(t for t in times.values() if t)
    json.dump({"capture": path, "gpu_total_s": total, "count": len(rows), "log": log, "actions": rows},
              open(out_path, "w"), indent=0)
    ctrl.Shutdown()
    cap.Shutdown()

try:
    main()
except Exception:
    json.dump({"error": traceback.format_exc(), "log": log}, open(out_path, "w"))
os._exit(0)
