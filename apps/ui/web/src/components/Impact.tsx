import { Card, Group, Progress, Stack, Text, Tooltip } from "@mantine/core";

import { Values } from "../options";

/**
 * A rough guide to what the chosen options cost: distant detail, build time, disk and VRAM, each
 * 0..100 relative to the heaviest setting. Weights come from cs-base measurements (2026-10-23):
 * Ultra trees with generated models roughly triple Tamriel's LOD4 object meshes, High rules and
 * 2048 px tiles add atlas pages, occlusion and tree models dominate build time.
 */
export function impact(v: Values) {
  const preset = { Low: 0, Medium: 1, High: 2 }[String(v.preset)] ?? 1;
  const ultra = v.treeLod === "ultra";
  const models = ultra && v.treeModels !== "off";
  const tile = { 256: 0, 512: 1, 1024: 2, 2048: 3 }[Number(v.atlasMaxTile)] ?? 2;
  const bb = Number(v.billboardScale ?? 1);
  const clamp = (n: number) => Math.max(5, Math.min(100, Math.round(n)));
  return {
    detail: clamp(25 + preset * 15 + (ultra ? 10 : 0) + (models ? 15 : 0) + tile * 5 + (bb - 1) * 10),
    time: clamp(30 + preset * 8 + (models ? 20 : 0) + (v.bc7 === "best" ? 25 : v.bc7 === "fast" ? -15 : 0) + (v.occlusion ? 15 : 0) + (v.texgen ? 8 : 0)),
    disk: clamp(20 + preset * 10 + (ultra ? 10 : 0) + (models ? 25 : 0) + tile * 8 + (bb - 1) * 10),
    vram: clamp(20 + preset * 12 + (models ? 20 : 0) + tile * 10 + (bb - 1) * 12),
  };
}

const ROWS: { key: keyof ReturnType<typeof impact>; label: string; color: string; help: string }[] = [
  { key: "detail", label: "Distant detail", color: "teal", help: "How much of the world shows at distance, and how close to the full models it looks." },
  { key: "time", label: "Build time", color: "blue", help: "Tamriel takes 5 to 40 minutes depending on these settings." },
  { key: "disk", label: "Disk", color: "grape", help: "Size of the output mod (a few GB up to ~15 GB with every season)." },
  { key: "vram", label: "VRAM / load", color: "orange", help: "Object LOD meshes and atlas pages the game keeps loaded." },
];

export function ImpactCard({ values }: { values: Values }) {
  const i = impact(values);
  return (
    <Card withBorder>
      <Text fw={600} size="sm" mb="xs">
        What these settings cost (rough guide)
      </Text>
      <Stack gap={8}>
        {ROWS.map((r) => (
          <Tooltip key={r.key} label={r.help} withArrow position="left">
            <Group gap="sm" wrap="nowrap">
              <Text size="xs" w={100}>
                {r.label}
              </Text>
              <Progress style={{ flex: 1 }} value={i[r.key]} color={r.color} size="lg" />
            </Group>
          </Tooltip>
        ))}
      </Stack>
    </Card>
  );
}
