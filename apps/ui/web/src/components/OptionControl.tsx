import { Group, NumberInput, SegmentedControl, Select, Slider, Stack, Switch, Text, TextInput, Tooltip } from "@mantine/core";

import { Option, Values } from "../options";

const ATLAS_TILES = [256, 512, 1024, 2048];

/** One option as a control with its explanation (and the chosen value's explanation). */
export function OptionControl({ option: o, values, onChange }: { option: Option; values: Values; onChange: (key: string, v: string | number | boolean) => void }) {
  const v = values[o.key] ?? o.default;
  const flag = (
    <Tooltip label={o.kind === "bool" && o.negate ? `${o.flag} when off` : o.flag} withArrow>
      <Text span size="xs" c="dimmed" ff="monospace">
        {o.flag}
      </Text>
    </Tooltip>
  );
  const header = (
    <Group justify="space-between" gap="xs" wrap="nowrap">
      <Text fw={600} size="sm">
        {o.label}
      </Text>
      {flag}
    </Group>
  );
  switch (o.kind) {
    case "bool":
      return (
        <Stack gap={4}>
          <Group justify="space-between" wrap="nowrap">
            <Switch checked={Boolean(v)} onChange={(e) => onChange(o.key, e.currentTarget.checked)} label={<Text fw={600} size="sm">{o.label}</Text>} />
            {flag}
          </Group>
          <Text size="xs" c="dimmed">
            {o.explain}
          </Text>
        </Stack>
      );
    case "choice": {
      const current = o.choices.find((c) => c.value === v);
      return (
        <Stack gap={6}>
          {header}
          {o.choices.length <= 4 ? (
            <SegmentedControl fullWidth size="xs" value={String(v)} onChange={(x) => onChange(o.key, x)} data={o.choices.map((c) => ({ value: c.value, label: c.label }))} />
          ) : (
            <Select value={String(v)} onChange={(x) => x && onChange(o.key, x)} data={o.choices.map((c) => ({ value: c.value, label: c.label }))} />
          )}
          <Text size="xs" c="dimmed">
            {current?.explain ?? o.explain}
          </Text>
        </Stack>
      );
    }
    case "number": {
      const n = Number(v);
      const label = o.format ? o.format(n) : String(n);
      if (o.key === "atlasMaxTile") {
        const idx = Math.max(0, ATLAS_TILES.indexOf(n));
        return (
          <Stack gap={6}>
            {header}
            <Slider min={0} max={ATLAS_TILES.length - 1} step={1} value={idx} label={null} onChange={(i) => onChange(o.key, ATLAS_TILES[i])} marks={ATLAS_TILES.map((t, i) => ({ value: i, label: `${t}` }))} mb="md" />
            <Text size="xs" c="dimmed">
              {label}. {o.explain}
            </Text>
          </Stack>
        );
      }
      return (
        <Stack gap={6}>
          {header}
          <Group gap="sm" wrap="nowrap">
            <Slider style={{ flex: 1 }} min={o.min} max={o.max} step={o.step} value={n} label={label} onChange={(x) => onChange(o.key, x)} />
            <NumberInput w={90} size="xs" min={o.min} max={o.max} step={o.step} value={n} onChange={(x) => typeof x === "number" && onChange(o.key, x)} />
          </Group>
          <Text size="xs" c="dimmed">
            {label}. {o.explain}
          </Text>
        </Stack>
      );
    }
    case "text":
      return (
        <Stack gap={6}>
          {header}
          <TextInput size="xs" value={String(v)} placeholder={o.placeholder} onChange={(e) => onChange(o.key, e.currentTarget.value)} spellCheck={false} />
          <Text size="xs" c="dimmed">
            {o.explain}
          </Text>
        </Stack>
      );
  }
}
