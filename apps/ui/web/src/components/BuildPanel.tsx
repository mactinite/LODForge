import { useMemo } from "react";
import { Alert, Badge, Button, Card, Code, Group, Loader, Progress, ScrollArea, Stack, Table, Tabs, Text, Title } from "@mantine/core";
import { BarChart } from "@mantine/charts";
import { IconAlertTriangle, IconCheck, IconPlayerStop } from "@tabler/icons-react";

import { BuildState } from "../state";

const fmt = (ms: number) => (ms >= 90000 ? `${(ms / 60000).toFixed(1)} min` : `${(ms / 1000).toFixed(1)} s`);

/** Progress, stage table, a per-stage timing chart and the log of a running or finished build. */
export function BuildPanel({ state, expected, onCancel }: { state: BuildState; expected: number; onCancel: () => void }) {
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
        .map((s) => ({ stage: `${s.world === "all" ? "" : s.world + " "}${s.stage}${s.season ? ` [${s.season}]` : ""}`, seconds: Math.round((s.ms ?? 0) / 100) / 10 }))
        .sort((a, b) => b.seconds - a.seconds)
        .slice(0, 12),
    [state.stages],
  );
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
      {failed.length > 0 && !state.running && (
        <Alert color="red" title="Stages that did not finish" icon={<IconAlertTriangle />}>
          {failed.map((f) => `${f.world} ${f.stage}${f.season ? ` [${f.season}]` : ""}`).join(", ")}. The errors are in the log.
        </Alert>
      )}
      <Tabs defaultValue="stages">
        <Tabs.List>
          <Tabs.Tab value="stages">Stages</Tabs.Tab>
          <Tabs.Tab value="timing">Timing</Tabs.Tab>
          <Tabs.Tab value="log">
            Log {state.errors.length > 0 && <Badge size="xs" color="red" ml={4}>{state.errors.length}</Badge>}
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
                    <Table.Td>{s.stage}</Table.Td>
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
          <ScrollArea h={320}>
            <Code block>{state.log.join("\n")}</Code>
          </ScrollArea>
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}
