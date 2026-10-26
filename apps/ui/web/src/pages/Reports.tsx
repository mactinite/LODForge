import { useEffect, useMemo, useState } from "react";
import { Alert, Badge, Card, Group, ScrollArea, Select, SimpleGrid, Stack, Table, Tabs, Text, TextInput, Title } from "@mantine/core";
import { BarChart, DonutChart } from "@mantine/charts";
import { IconSearch } from "@tabler/icons-react";

import { invoke } from "../backend";
import { Output } from "../state";

interface TreeRow {
  plugin: string;
  editorId: string;
  model: string;
  refs: number;
  lod4: string;
  lod8: string;
  lod16: string;
  note: string;
  candidate: boolean;
}

function parseCsv(text: string): string[][] {
  const out: string[][] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line) continue;
    const cells: string[] = [];
    let cur = "";
    let q = false;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      if (q) {
        if (ch === '"' && line[i + 1] === '"') (cur += '"'), i++;
        else if (ch === '"') q = false;
        else cur += ch;
      } else if (ch === '"') q = true;
      else if (ch === ",") (cells.push(cur), (cur = ""));
      else cur += ch;
    }
    cells.push(cur);
    out.push(cells);
  }
  return out;
}

const KIND_COLORS: Record<string, string> = { "3d": "teal.6", billboard: "yellow.6", full: "grape.6", none: "gray.6" };
const KIND_LABEL: Record<string, string> = { "3d": "3D model", billboard: "Billboard", full: "Full model", none: "No LOD" };

/** The last build's reports per worldspace: trees (every build) and impostors (runtime builds). */
export function Reports({ worlds, output, instance }: { worlds: string[]; output: Output; instance: string }) {
  const [world, setWorld] = useState<string | null>(worlds[0] ?? null);
  return (
    <Stack>
      <Group justify="space-between">
        <Title order={3}>Reports</Title>
        <Select data={worlds} value={world} onChange={setWorld} placeholder="Worldspace" w={240} />
      </Group>
      <Tabs defaultValue="trees">
        <Tabs.List>
          <Tabs.Tab value="trees">Trees</Tabs.Tab>
          <Tabs.Tab value="impostors">Impostors (runtime)</Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="trees" pt="sm">
          <TreeReport world={world} output={output} instance={instance} />
        </Tabs.Panel>
        <Tabs.Panel value="impostors" pt="sm">
          <ImpostorReport world={world} output={output} instance={instance} />
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}

interface ReportProps {
  world: string | null;
  output: Output;
  instance: string;
}

function readReport(instance: string, output: Output, name: string) {
  return invoke<string>("read_report", { instance, out: output.dest === "folder" ? output.folder : null, mo2_mod: output.dest === "mo2" ? output.modName : null, name });
}

/** What LOD every tree got. */
function TreeReport({ world, output, instance }: ReportProps) {
  const [rows, setRows] = useState<TreeRow[] | null>(null);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!world) return;
    setRows(null);
    readReport(instance, output, `LODForge_TreeReport_${world}.csv`)
      .then((t) => {
        const [, ...data] = parseCsv(t);
        setRows(data.map((c) => ({ plugin: c[0], editorId: c[2], model: c[3], refs: Number(c[5]) || 0, lod4: c[6], lod8: c[7], lod16: c[8], note: c[11] ?? "", candidate: c[12] === "yes" })));
        setError("");
      })
      .catch((e) => setError(String(e)));
  }, [world, instance, output.dest, output.folder, output.modName]);

  const stats = useMemo(() => {
    if (!rows) return null;
    const total = rows.reduce((a, r) => a + r.refs, 0);
    const by = (k: "lod4" | "lod8" | "lod16") => Object.entries(rows.reduce<Record<string, number>>((m, r) => ((m[r[k]] = (m[r[k]] ?? 0) + r.refs), m), {})).map(([name, value]) => ({ name: KIND_LABEL[name] ?? name, value, color: KIND_COLORS[name] ?? "blue.6" }));
    const top = rows.filter((r) => r.lod4 !== "3d").slice(0, 10).map((r) => ({ tree: r.editorId || r.model.split("\\").pop(), references: r.refs }));
    return { total, lod4: by("lod4"), lod8: by("lod8"), lod16: by("lod16"), top };
  }, [rows]);

  const shown = (rows ?? []).filter((r) => !query || `${r.editorId} ${r.model} ${r.plugin} ${r.note}`.toLowerCase().includes(query.toLowerCase()));

  return (
    <Stack>
      <Text size="sm" c="dimmed">
        What every placed tree gets at each LOD level in the last build ({output.dest === "mo2" ? `mod "${output.modName}"` : output.folder}): a 3D model, a billboard, the full model or nothing. Trees placed often without a 3D model are where generated models help most.
      </Text>
      {error && <Alert color="yellow">No tree report for {world} in this output yet. Build in DynDOLOD mode with object LOD first. ({error})</Alert>}
      {stats && (
        <>
          <SimpleGrid cols={{ base: 1, md: 4 }}>
            <Card withBorder>
              <Text size="xs" c="dimmed">
                Tree references
              </Text>
              <Text fw={700} size="xl">
                {stats.total.toLocaleString()}
              </Text>
              <Text size="xs" c="dimmed">
                {rows!.length} tree bases
              </Text>
            </Card>
            {(["lod4", "lod8", "lod16"] as const).map((k) => (
              <Card withBorder key={k}>
                <Text size="xs" fw={600} ta="center">
                  {k.toUpperCase()}
                </Text>
                <Group justify="center">
                  <DonutChart size={120} thickness={20} data={stats[k]} withTooltip tooltipDataSource="segment" />
                </Group>
              </Card>
            ))}
          </SimpleGrid>
          <Card withBorder>
            <Text fw={600} size="sm" mb="xs">
              Most placed trees without a 3D LOD4 model
            </Text>
            <BarChart h={300} data={stats.top} dataKey="tree" orientation="vertical" yAxisProps={{ width: 200 }} series={[{ name: "references", color: "yellow.6" }]} />
          </Card>
          <Card withBorder p={0}>
            <Group p="xs">
              <TextInput size="xs" leftSection={<IconSearch size={14} />} placeholder="Filter trees" value={query} onChange={(e) => setQuery(e.currentTarget.value)} style={{ flex: 1 }} />
              <Badge variant="light">{shown.length} trees</Badge>
            </Group>
            <ScrollArea h={360}>
              <Table stickyHeader striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Tree</Table.Th>
                    <Table.Th>Plugin</Table.Th>
                    <Table.Th ta="right">Refs</Table.Th>
                    <Table.Th>LOD4</Table.Th>
                    <Table.Th>LOD8</Table.Th>
                    <Table.Th>LOD16</Table.Th>
                    <Table.Th>Note</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {shown.slice(0, 400).map((r, i) => (
                    <Table.Tr key={i}>
                      <Table.Td>
                        <Text size="xs" fw={600}>
                          {r.editorId}
                        </Text>
                        <Text size="xs" c="dimmed">
                          {r.model}
                        </Text>
                      </Table.Td>
                      <Table.Td>
                        <Text size="xs">{r.plugin}</Text>
                      </Table.Td>
                      <Table.Td ta="right">{r.refs.toLocaleString()}</Table.Td>
                      {[r.lod4, r.lod8, r.lod16].map((k, j) => (
                        <Table.Td key={j}>
                          <Badge size="xs" color={(KIND_COLORS[k] ?? "blue.6").split(".")[0]} variant="light">
                            {KIND_LABEL[k] ?? k}
                          </Badge>
                        </Table.Td>
                      ))}
                      <Table.Td>
                        <Text size="xs" c="dimmed">
                          {r.note}
                        </Text>
                      </Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </ScrollArea>
          </Card>
        </>
      )}
    </Stack>
  );
}

interface ImpostorRow {
  type: string;
  model: string;
  refs: number;
  grid: number;
  frame: number;
  atlas: number;
  quad: number;
  bakeMs: number;
  cached: boolean;
  bytes: number;
  coverage: number;
}

const mb = (b: number) => `${(b / 1048576).toFixed(b >= 1048576 * 100 ? 0 : 1)} MB`;

/** The octahedral tree impostors a runtime build baked: one per tree type and season variant. */
function ImpostorReport({ world, output, instance }: ReportProps) {
  const [rows, setRows] = useState<ImpostorRow[] | null>(null);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!world) return;
    setRows(null);
    readReport(instance, output, `LODForge_ImpostorReport_${world}.csv`)
      .then((t) => {
        const [head, ...data] = parseCsv(t);
        const col = (n: string) => head.indexOf(n);
        const num = (c: string[], n: string) => Number(c[col(n)]) || 0;
        setRows(
          data
            .map((c) => ({ type: c[col("type")], model: c[col("model")], refs: num(c, "references"), grid: num(c, "grid"), frame: num(c, "frame_px"), atlas: num(c, "atlas_px"), quad: num(c, "quad_size"), bakeMs: num(c, "bake_ms"), cached: c[col("cached")] === "true", bytes: num(c, "bytes"), coverage: num(c, "coverage") }))
            .sort((a, b) => b.refs - a.refs),
        );
        setError("");
      })
      .catch((e) => setError(String(e)));
  }, [world, instance, output.dest, output.folder, output.modName]);

  const stats = useMemo(() => {
    if (!rows) return null;
    const refs = rows.reduce((a, r) => a + r.refs, 0);
    const bytes = rows.reduce((a, r) => a + r.bytes, 0);
    const cached = rows.filter((r) => r.cached).length;
    const bake = rows.filter((r) => !r.cached).reduce((a, r) => a + r.bakeMs, 0);
    const frames = Object.entries(rows.reduce<Record<string, number>>((m, r) => ((m[`${r.frame} px`] = (m[`${r.frame} px`] ?? 0) + 1), m), {})).map(([frame, types]) => ({ frame, types }));
    return { refs, bytes, cached, bake, frames };
  }, [rows]);

  const shown = (rows ?? []).filter((r) => !query || `${r.type} ${r.model}`.toLowerCase().includes(query.toLowerCase()));

  return (
    <Stack>
      <Text size="sm" c="dimmed">
        Octahedral impostors baked for the LODForge Runtime: every tree type the runtime draws gets a grid of views (frames) on the impostor pages. The bake cache keeps unchanged trees between builds.
      </Text>
      {error && <Alert color="yellow">No impostor report for {world} in this output. Impostors are baked by runtime builds (LODForge Runtime on). ({error})</Alert>}
      {stats && (
        <>
          <SimpleGrid cols={{ base: 2, md: 4 }}>
            <Card withBorder>
              <Text size="xs" c="dimmed">
                Impostor types
              </Text>
              <Text fw={700} size="xl">
                {rows!.length}
              </Text>
              <Text size="xs" c="dimmed">
                {stats.refs.toLocaleString()} tree references
              </Text>
            </Card>
            <Card withBorder>
              <Text size="xs" c="dimmed">
                Texture data
              </Text>
              <Text fw={700} size="xl">
                {mb(stats.bytes)}
              </Text>
              <Text size="xs" c="dimmed">
                albedo + normal frames, before packing
              </Text>
            </Card>
            <Card withBorder>
              <Text size="xs" c="dimmed">
                From the bake cache
              </Text>
              <Text fw={700} size="xl">
                {stats.cached} / {rows!.length}
              </Text>
              <Text size="xs" c="dimmed">
                {(stats.bake / 1000).toFixed(1)} s baking the rest
              </Text>
            </Card>
            <Card withBorder>
              <Text size="xs" c="dimmed">
                Frame sizes
              </Text>
              <BarChart h={70} data={stats.frames} dataKey="frame" series={[{ name: "types", color: "teal.6" }]} withYAxis={false} gridAxis="none" />
            </Card>
          </SimpleGrid>
          <Card withBorder p={0}>
            <Group p="xs">
              <TextInput size="xs" leftSection={<IconSearch size={14} />} placeholder="Filter types" value={query} onChange={(e) => setQuery(e.currentTarget.value)} style={{ flex: 1 }} />
              <Badge variant="light">{shown.length} types</Badge>
            </Group>
            <ScrollArea h={360}>
              <Table stickyHeader striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Type</Table.Th>
                    <Table.Th ta="right">Refs</Table.Th>
                    <Table.Th ta="right">Frames</Table.Th>
                    <Table.Th ta="right">Size</Table.Th>
                    <Table.Th ta="right">Coverage</Table.Th>
                    <Table.Th ta="right">Data</Table.Th>
                    <Table.Th>Bake</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {shown.slice(0, 400).map((r, i) => (
                    <Table.Tr key={i}>
                      <Table.Td>
                        <Text size="xs" fw={600}>
                          {r.type}
                        </Text>
                        <Text size="xs" c="dimmed">
                          {r.model}
                        </Text>
                      </Table.Td>
                      <Table.Td ta="right">{r.refs.toLocaleString()}</Table.Td>
                      <Table.Td ta="right">
                        {r.grid}×{r.grid} @ {r.frame} px
                      </Table.Td>
                      <Table.Td ta="right">{Math.round(r.quad)}</Table.Td>
                      <Table.Td ta="right">{Math.round(r.coverage * 100)}%</Table.Td>
                      <Table.Td ta="right">{mb(r.bytes)}</Table.Td>
                      <Table.Td>
                        {r.cached ? (
                          <Badge size="xs" variant="light" color="gray">
                            cached
                          </Badge>
                        ) : (
                          <Text size="xs">{(r.bakeMs / 1000).toFixed(1)} s</Text>
                        )}
                      </Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </ScrollArea>
          </Card>
        </>
      )}
    </Stack>
  );
}
