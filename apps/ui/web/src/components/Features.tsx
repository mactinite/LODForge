import { ReactNode } from "react";
import { Alert, Badge, Card, Group, SimpleGrid, Stack, Switch, Text } from "@mantine/core";

import { ProfileInfo } from "../backend";
import { Values, isRuntime } from "../options";

type Set = (k: string, v: string | number | boolean) => void;
type Control = (key: string) => ReactNode;

/** A feature the build can add: title, status badges, what it does, an on/off switch and its settings while on. */
export function FeatureCard(p: { title: string; badges?: ReactNode; description: ReactNode; on: boolean; onToggle: (on: boolean) => void; disabled?: boolean; children?: ReactNode; notes?: ReactNode }) {
  return (
    <Card withBorder style={{ opacity: p.disabled ? 0.6 : 1 }}>
      <Group justify="space-between" wrap="nowrap" align="flex-start">
        <div>
          <Group gap="xs">
            <Text fw={600}>{p.title}</Text>
            {p.badges}
          </Group>
          <Text size="xs" c="dimmed" maw={720} mt={2}>
            {p.description}
          </Text>
        </div>
        <Switch checked={p.on} disabled={p.disabled} onChange={(e) => p.onToggle(e.currentTarget.checked)} aria-label={p.title} />
      </Group>
      {p.notes}
      {p.on && p.children && (
        <SimpleGrid cols={{ base: 1, md: 2 }} mt="sm" spacing="lg">
          {p.children}
        </SimpleGrid>
      )}
    </Card>
  );
}

const status = (ok: boolean | undefined, yes: string, no: string) =>
  ok === undefined ? null : (
    <Badge size="xs" color={ok ? "green" : "yellow"} variant="light">
      {ok ? yes : no}
    </Badge>
  );

/** The optional parts of a build, each on its own card (Quality step). */
export function Features({ info, values, set, setMany, control }: { info: ProfileInfo | null; values: Values; set: Set; setMany: (v: Values) => void; control: Control }) {
  const runtime = isRuntime(values);
  // In runtime mode, "DynDOLOD plugins off" keeps the runtime's own dynamic LOD (.dyn.bin);
  // in a classic build it means no dynamic LOD at all.
  const dynOn = values.plugins !== false && !(runtime && values.runtimeNoDyndolodPlugins === true);
  const setDyn = (on: boolean) => setMany(runtime ? { ...values, plugins: true, runtimeNoDyndolodPlugins: !on } : { ...values, plugins: on, runtimeNoDyndolodPlugins: false });
  const grassOn = values.grass !== "off";
  return (
    <Stack>
      <FeatureCard
        title="LODForge Runtime"
        badges={status(info?.runtime_plugin, "plugin installed", "plugin not in this profile")}
        description="Distant trees become octahedral impostors with 3D tree models near you; repeated objects and large references are drawn instanced by the SKSE plugin, and far grass comes from the grass cache. Adds the impostor bake and runtime stages to the build."
        on={runtime}
        // The DynDOLOD card's on/off carries over: its "off" is --no-plugins in a classic build and
        // --runtime-no-dyndolod-plugins in a runtime build.
        onToggle={(on) => setMany({ ...values, runtime: on ? "lodforge" : "none", plugins: true, runtimeNoDyndolodPlugins: false, ...(dynOn ? {} : on ? { runtimeNoDyndolodPlugins: true } : { plugins: false }) })}
        notes={
          <>
            {runtime && info?.runtime_plugin === false && (
              <Alert color="yellow" mt="sm" variant="light">
                This profile has no SKSE\Plugins\LODForgeRuntime.dll (or its mod is not enabled). Without it the game shows no trees and no instanced objects from this output.
              </Alert>
            )}
            {runtime && values.runtimeGrass !== false && info?.grass_cache === 0 && !String(values.runtimeGrassCache ?? "").trim() && (
              <Text size="xs" c="yellow.5" mt="xs">
                No NGIO grass cache in the load order, so no far grass will be written. Precache grass, or set a cache folder in the Advanced tab.
              </Text>
            )}
          </>
        }
      >
        {control("impostorUnitsPerPx")}
        {control("runtimeGrass")}
      </FeatureCard>

      <FeatureCard
        title="DynDOLOD dynamic LOD"
        badges={runtime ? <Badge size="xs" variant="light" color="gray">optional with the runtime</Badge> : status(info?.dyndolod_dll, "DynDOLOD DLL installed", "DynDOLOD DLL not in this profile")}
        description={
          runtime
            ? "DynDOLOD.esm/.esp and the DynDOLOD DLL NG data, beside the runtime. Off: the runtime draws dynamic LOD itself from its .dyn.bin files and no DynDOLOD plugins are written (the runtime's dynamic LOD is newer; keep this on until it covers your list). Child copies and glowing windows do not depend on this."
            : "DynDOLOD.esm/.esp and the DynDOLOD DLL NG data: LOD that changes in game (doors, lights, destroyed or quest-enabled objects) and the large reference hand-off. Off: static object LOD only."
        }
        on={dynOn}
        onToggle={setDyn}
        notes={
          !runtime &&
          dynOn &&
          info?.dyndolod_dll === false && (
            <Text size="xs" c="yellow.5" mt="xs">
              The DynDOLOD DLL NG is not in this profile: the plugins are written, but dynamic LOD needs the DLL (or DynDOLOD's Papyrus scripts) in game.
            </Text>
          )
        }
      >
        {control("largeRefWorkarounds")}
      </FeatureCard>

      <FeatureCard
        title="Glowing windows"
        description={
          runtime
            ? "Window glow models in the distant LOD of towns and buildings. The LODForge Runtime lights them itself by the hour of the day, with or without the DynDOLOD plugins."
            : "Window glow models in the distant LOD of towns and buildings (DynDOLOD's Windows option)."
        }
        on={values.windows === true}
        onToggle={(on) => set("windows", on)}
        notes={
          runtime &&
          values.windows === true && (
            <Text size="xs" c="dimmed" mt="xs">
              New with the runtime: its window glow has not been checked in game yet.
            </Text>
          )
        }
      />

      <FeatureCard
        title="Occlusion"
        description="Occlusion.esp: precomputed visibility from the new terrain and object LOD, so the game skips cells hidden behind hills and mountains. Written for every build style; computed in the background while the other stages run."
        on={values.occlusion !== false}
        onToggle={(on) => set("occlusion", on)}
      />

      {!runtime && (
        <FeatureCard
          title="Grass LOD"
          description="Distant grass beyond the game's grass radius. The LODForge Runtime has its own far grass instead."
          on={grassOn}
          onToggle={(on) => set("grass", on ? "go" : "off")}
        >
          {control("grass")}
          {control("grassDensity")}
        </FeatureCard>
      )}
    </Stack>
  );
}
