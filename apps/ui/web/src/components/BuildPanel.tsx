import { useMemo, useState } from "react";
import { Alert, Badge, Box, Button, Card, Group, Loader, Progress, ScrollArea, SegmentedControl, Stack, Table, Tabs, Text, Title } from "@mantine/core";
import { BarChart } from "@mantine/charts";
import { IconAlertTriangle, IconCheck, IconPlayerStop } from "@tabler/icons-react";

import { BuildState } from "../state";
import { LogLine, STAGE_LABELS, stageLabel } from "../stages";

const fmt = (ms: number) => (ms >= 90000 ? `${(ms / 60000).toFixed(1)} min` : `${(ms / 1000).toFixed(1)} s`);

/** Progress, stage table, a per-stage timing chart and the log of a running or finished build. */
export function BuildPanel({ state, expected, onCancel }: { state: BuildState; expected: number; onCancel: () => void }) {
  const [logFilter, setLogFilter] = useState<"all" | "problems">("all");
  const done = state.stages.filter((s) => s.state === "done").length;
  const failed = state.stages.filter((s) => s.state === "failed");
  const elapsed = (state.finishedAt ?? Date.now()) - state.startedAt;
  const pct = state.running ? Math.min(98, (100 * done) / Math.max(1, expected)) : 100;
  const ok = !state.running && state.exitCode === 0 && failed.length === 0;
  // Slowest stages first, as a bar chart (minutes).
  const chart = useMemo(
    () =>
      state.stages
        .filter((s) => s.ms !== null)
        .map((s) => ({ stage: `${s.world === "all" ? "" : s.world + " "}${stageLabel(s.stage)}${s.season ? ` [${s.season}]` : ""}`, seconds: Math.round((s.ms ?? 0) / 100) / 10 }))
        .sort((a, b) => b.seconds - a.seconds)
        .slice(0, 12),
    [state.stages],
  );
  // Running stages, for the line under the progress bar (the terrain lane runs beside the object stages).
  const running = state.stages.filter((s) => s.state === "running");
  const runtimeMs = state.stages.filter((s) => STAGE_LABELS[s.stage]?.runtime && s.ms !== null).reduce((a, s) => a + (s.ms ?? 0), 0);
  if (!state.startedAt) return null;
  return (
    <Stack>
      <Group justify="space-between">
        <Group>
          {state.running ? <Loader size="sm" /> : ok ? <IconCheck color="var(--mantine-color-green-6)" /> : <IconAlertTriangle color="var(--mantine-color-red-6)" />}
          <Title order={3}>{state.running ? "Building…" : ok ? "Build complete" : "Finished with errors"}</Title>
        </Group>
        <Group>
          <Text c="dimmed" size="sm">
            {done} stage(s) · {fmt(elapsed)}
          </Text>
          {state.running && (
            <Button color="red" variant="light" leftSection={<IconPlayerStop size={16} />} onClick={onCancel}>
              Cancel
            </Button>
          )}
        </Group>
      </Group>
      <Progress value={pct} animated={state.running} color={ok || state.running ? "orange" : "red"} />
      {state.running && running.length > 0 && (
        <Text size="xs" c="dimmed">
          Now: {running.map((s) => `${s.world === "all" ? "" : s.world + " "}${stageLabel(s.stage)}${s.season ? ` [${s.season}]` : ""}`).join(" · ")}
        </Text>
      )}
      {runtimeMs > 0 && (
        <Text size="xs" c="dimmed">
          LODForge Runtime stages so far: {fmt(runtimeMs)}
        </Text>
      )}
      {failed.length > 0 && !state.running && (
        <Alert color="red" title="Stages that did not finish" icon={<IconAlertTriangle />}>
          {failed.map((f) => `${f.world} ${stageLabel(f.stage)}${f.season ? ` [${f.season}]` : ""}`).join(", ")}. The errors are in the log.
        </Alert>
      )}
      <Tabs defaultValue="stages">
        <Tabs.List>
          <Tabs.Tab value="stages">Stages</Tabs.Tab>
          <Tabs.Tab value="timing">Timing</Tabs.Tab>
          <Tabs.Tab value="log">
            Log
            {state.errors > 0 && (
              <Badge size="xs" color="red" ml={4} title="errors">
                {state.errors}
              </Badge>
            )}
            {state.warnings > 0 && (
              <Badge size="xs" color="yellow" ml={4} title="warnings">
                {state.warnings}
              </Badge>
            )}
          </Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="stages" pt="sm">
          <ScrollArea h={320}>
            <Table striped highlightOnHover stickyHeader>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Worldspace</Table.Th>
                  <Table.Th>Stage</Table.Th>
                  <Table.Th>Season</Table.Th>
                  <Table.Th ta="right">Time</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {state.stages.map((s, i) => (
                  <Table.Tr key={s.key + i}>
                    <Table.Td>{s.world}</Table.Td>
                    <Table.Td>
                      {stageLabel(s.stage)}
                      {STAGE_LABELS[s.stage]?.runtime && (
                        <Badge size="xs" variant="light" color="teal" ml={6}>
                          runtime
                        </Badge>
                      )}
                    </Table.Td>
                    <Table.Td>{s.season}</Table.Td>
                    <Table.Td ta="right">{s.state === "running" ? <Loader size="xs" /> : s.state === "failed" ? <Badge color="red">failed</Badge> : fmt(s.ms ?? 0)}</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </ScrollArea>
        </Tabs.Panel>
        <Tabs.Panel value="timing" pt="sm">
          <Card withBorder>
            <Text size="sm" c="dimmed" mb="xs">
              Slowest stages (seconds)
            </Text>
            {chart.length ? <BarChart h={Math.max(160, chart.length * 28)} data={chart} dataKey="stage" orientation="vertical" yAxisProps={{ width: 220 }} series={[{ name: "seconds", color: "orange.6" }]} /> : <Text c="dimmed">No finished stages yet.</Text>}
          </Card>
        </Tabs.Panel>
        <Tabs.Panel value="log" pt="sm">
          <Group justify="space-between" mb="xs">
            <SegmentedControl
              size="xs"
              value={logFilter}
              onChange={(v) => setLogFilter(v as "all" | "problems")}
              data={[
                { value: "all", label: "Everything" },
                { value: "problems", label: `Errors & warnings (${state.errors + state.warnings})` },
              ]}
            />
            <Text size="xs" c="dimmed">
              Notes are plain text; only red lines mean a stage failed.
            </Text>
          </Group>
          <LogView lines={state.log} problemsOnly={logFilter === "problems"} />
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}

const LEVEL_STYLE: Record<string, { color: string; bg: string; mark: string }> = {
  error: { color: "var(--mantine-color-red-text)", bg: "rgba(250, 82, 82, 0.14)", mark: "var(--mantine-color-red-6)" },
  warning: { color: "var(--mantine-color-yellow-text)", bg: "rgba(250, 176, 5, 0.10)", mark: "var(--mantine-color-yellow-6)" },
};

/** The build log: JSON stage events left out, errors red and warnings yellow (the last 3000 lines unless filtered). */
function LogView({ lines, problemsOnly }: { lines: LogLine[]; problemsOnly: boolean }) {
  const shown = useMemo(() => {
    const text = lines.filter((l) => !l.text.startsWith('{"event"'));
    const pick = problemsOnly ? text.filter((l) => l.level !== "info") : text;
    return pick.length > 3000 ? pick.slice(pick.length - 3000) : pick;
  }, [lines, problemsOnly]);
  return (
    <ScrollArea h={320} type="auto" style={{ background: "var(--mantine-color-default-hover)", borderRadius: 4 }}>
      <Box p={6} ff="monospace" fz={12} style={{ whiteSpace: "pre" }}>
        {shown.length === 0 && <Text size="xs" c="dimmed">{problemsOnly ? "No errors or warnings." : "No output yet."}</Text>}
        {shown.map((l, i) => {
          const st = LEVEL_STYLE[l.level];
          return (
            <div key={i} style={st ? { color: st.color, background: st.bg, borderLeft: `3px solid ${st.mark}`, paddingLeft: 6 } : { paddingLeft: 9 }}>
              {l.text}
            </div>
          );
        })}
      </Box>
    </ScrollArea>
  );
}
