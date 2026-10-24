# Far grass `.lfg`: sizes and how to get a grass cache

Format: [instance-formats.md](instance-formats.md) section 7. Writer: `apps/cli/src/grass_far.rs`
(`build --runtime lodforge`, or `lodforge runtime-grass` on its own).

## Measured: Tamriel (2026-10-24)

Input: a complete Tamriel cache in NGIO's `.cgid` format (default season plus SPR/SUM/AUT/WIN infixes)
from the 2026-10-19 determinism run (`D:\Modlists\LODForge-out\determinism-20261019\A\work\grass\Tamriel`,
5,694 default-season files, 7.0 GB for all seasons). Converted against the skyrim-cs-base load
order; every grass type of the default season was found in it by model and object id (4 autumn
types were not and are stored with their cache form ids, `GTYP.plugin = 0xFFFF`).

```
lodforge runtime-grass --instance D:\Modlists\skyrim-cs-base --world Tamriel --seasons AUT ^
  --cache <cache dir> --out <Data dir> --zstd 3
```

| | Default season | AUT |
|---|---|---|
| Cells with grass | 5,617 | 6,283 |
| Instances | 29,688,553 | 40,270,412 |
| `.cgid` input | 960.6 MB | 1,299.9 MB |
| Conversion (16 threads, warm cache) | 3.9 s | 4.0 s |

File size if the runtime kept only density level `k` (the first `prefix[k]` records of every
cell), raw and with zstd level 3 per cell:

| Level k | Fraction | Default raw | Default zstd | AUT raw | AUT zstd |
|---|---|---|---|---|---|
| 0 | 1 | 297.4 MB | 235.8 MB | 403.3 MB | 317.1 MB |
| 1 | 1/2 | 149.0 MB | 118.7 MB | 202.0 MB | 159.5 MB |
| 2 | 1/4 | 74.8 MB | 59.9 MB | 101.3 MB | 80.4 MB |
| 3 | 1/8 | 37.6 MB | 30.5 MB | 51.0 MB | 40.9 MB |
| 4 | 1/16 | 19.1 MB | 15.7 MB | 25.8 MB | 21.0 MB |
| 5 | 1/32 | 9.8 MB | 8.2 MB | 13.2 MB | 10.9 MB |
| 6 | 1/64 | 5.2 MB | 4.5 MB | 6.9 MB | 5.9 MB |
| 7 | 1/128 | 2.8 MB | 2.6 MB | 3.7 MB | 3.3 MB |

The whole file is level 0 (one file serves every density; the runtime reads prefixes). That is
10.0 bytes per instance raw; zstd saves only ~21% because records are in hash order (random
positions). A cell averages ~5,300 instances = 53 KB raw.

What this means for the runtime (estimates, not measured in game): a ring of ~10 cells beyond a
5x5 loaded grid is ~21x21 - 5x5 = 416 cells; at level 2 (1/4 density) that is ~416 x 13 KB =
5.5 MB of instance data, at level 0 for the inner ring (~96 cells) ~5 MB more. NGIO's extended
grass costs +3 GB RAM for comparison.

## Getting a cache

LODForge does not generate grass (the generator was removed from `main`, docs/licensing.md); the
`.lfg` is converted from a No Grass In Objects NG cache:

1. Install No Grass In Objects NG (and Grass Cache Helper NG for Seasons of Skyrim season caches).
2. Precache the worldspaces in game: put `PrecacheGrass.txt` next to `SkyrimSE.exe` and start the
   game; NGIO walks every cell, writes `Grass\<world>x<X>y<Y>.cgid` and exits (it crashes now and
   then; starting again continues). `scripts/grass/Run-GrassPrecache.ps1` automates this for an
   MO2 profile, including per-season runs (`-Seasons Default,WIN,...`, `-TargetMod`).
3. Leave the cache in the load order (`Grass\` through MO2's VFS, the default source) or point
   `--runtime-grass-cache <dir>` at a folder of `.cgid` files.

Caches name GRAS records by load-order form id; the converter matches each cache type to the
current load order by model and object id, as `--grass bto` does, so a cache made before a plugin
was added still converts.

skyrim-cs-base has no grass cache installed (2026-10-24); the measurements above used the
skyrim-cs cache from the determinism run.
