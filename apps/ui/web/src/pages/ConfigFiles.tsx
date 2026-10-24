import { useEffect, useMemo, useState } from "react";
import { ActionIcon, Alert, Badge, Button, Card, Group, NavLink, ScrollArea, Select, SimpleGrid, Stack, Switch, Table, Text, TextInput, Textarea, Title, Tooltip } from "@mantine/core";
import { DonutChart } from "@mantine/charts";
import { notifications } from "@mantine/notifications";
import { IconDeviceFloppy, IconFileText, IconPlus, IconRestore, IconSearch, IconTrash } from "@tabler/icons-react";

import { ConfigFile, invoke } from "../backend";
import { FORMATS, Line, joinFile, levelKind, parseFile } from "../configFormats";

/** The flat config files LODForge reads, as tables. Edits are saved into the LODForge Config overlay mod. */
export function ConfigFiles({ ready }: { ready: boolean }) {
  const [files, setFiles] = useState<ConfigFile[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [all, setAll] = useState(false);

  const refresh = async () => {
    try {
      setFiles(await invoke<ConfigFile[]>("config_files"));
    } catch (e) {
      notifications.show({ color: "red", title: "Config files", message: String(e) });
    }
  };
  useEffect(() => {
    if (ready) refresh();
  }, [ready]);

  if (!ready) return <Alert>Scan an instance first (Guided, step 1): config files are read from its DynDOLOD folder.</Alert>;
  const shown = files.filter((f) => (all || f.used || f.source === "overlay") && f.name.toLowerCase().includes(filter.toLowerCase()));
  const groups = [...new Set(shown.map((f) => f.name.split("\\")[0]))];
  const file = files.find((f) => f.name === selected) ?? null;

  return (
    <Group align="flex-start" wrap="nowrap" gap="lg">
      <Stack w={320} gap="xs" style={{ flexShrink: 0 }}>
        <TextInput size="xs" leftSection={<IconSearch size={14} />} placeholder="Filter files" value={filter} onChange={(e) => setFilter(e.currentTarget.value)} />
        <Switch size="xs" checked={all} onChange={(e) => setAll(e.currentTarget.checked)} label={`Also DynDOLOD/TexGen-only files (${files.filter((f) => !f.used).length})`} />
        <ScrollArea h={560}>
          {groups.map((g) => (
            <div key={g}>
              <Text size="xs" c="dimmed" tt="uppercase" mt="sm" mb={4}>
                {g}
              </Text>
              {shown
                .filter((f) => f.name.startsWith(g + "\\"))
                .map((f) => (
                  <NavLink
                    key={f.name}
                    active={f.name === selected}
                    onClick={() => setSelected(f.name)}
                    leftSection={<IconFileText size={14} />}
                    label={<Text size="xs">{f.name.split("\\").slice(1).join("\\").replace(/^DynDOLOD_SSE_/, "")}</Text>}
                    description={all && !f.used ? "not read by LODForge" : undefined}
                    rightSection={f.source === "overlay" ? <Badge size="xs" color="orange">edited</Badge> : f.source === "missing" ? <Badge size="xs" color="gray">new</Badge> : null}
                  />
                ))}
            </div>
          ))}
        </ScrollArea>
      </Stack>
      <div style={{ flex: 1, minWidth: 0 }}>{file ? <FileEditor key={file.name} file={file} onSaved={refresh} /> : <Overview files={files} />}</div>
    </Group>
  );
}

function Overview({ files }: { files: ConfigFile[] }) {
  const edited = files.filter((f) => f.source === "overlay").length;
  return (
    <Stack>
      <Title order={3}>Config files</Title>
      <Text c="dimmed" size="sm">
        The lists and rules DynDOLOD's configuration gives LODForge: which references are ignored, how child worldspaces copy into their parents, which vanilla meshes were replaced, and the LOD rules per mesh. Pick a file to see it as a table.
      </Text>
      <Alert variant="light" title="Where edits go">
        Saving writes a copy into the <b>LODForge Config</b> mod (mods\LODForge Config\DynDOLOD\…). LODForge reads that copy before DynDOLOD's own file; DynDOLOD's install is never changed. Reset deletes the copy.
      </Alert>
      <Group>
        <Badge size="lg" variant="light">{files.filter((f) => f.used).length} files LODForge reads</Badge>
        <Badge size="lg" variant="light" color="orange">{edited} edited</Badge>
      </Group>
    </Stack>
  );
}

function FileEditor({ file, onSaved }: { file: ConfigFile; onSaved: () => void }) {
  const format = FORMATS[file.format];
  const [text, setText] = useState<string | null>(null);
  const [lines, setLines] = useState<Line[]>([]);
  const [dirty, setDirty] = useState(false);
  const [query, setQuery] = useState("");
  const [section, setSection] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("read_config", { name: file.name }).then((t) => {
      setText(t);
      if (format) setLines(parseFile(t, format));
    });
  }, [file.name, format]);

  const save = async () => {
    const out = format ? joinFile(lines, format) : text ?? "";
    await invoke("write_config", { name: file.name, text: out });
    setDirty(false);
    notifications.show({ color: "green", title: "Saved", message: `${file.name} saved to the LODForge Config mod.` });
    onSaved();
  };
  const reset = async () => {
    await invoke("reset_config", { name: file.name });
    const t = await invoke<string>("read_config", { name: file.name });
    setText(t);
    if (format) setLines(parseFile(t, format));
    setDirty(false);
    onSaved();
  };

  const sections = useMemo(() => [...new Set(lines.filter((l) => l.kind === "row").map((l) => l.section))], [lines]);
  const rows = lines.map((l, i) => ({ l, i })).filter(({ l }) => l.kind === "row" && (!section || l.section === section) && (!query || (l.kind === "row" && l.cells.join(" ").toLowerCase().includes(query.toLowerCase()))));
  const setCell = (i: number, c: number, v: string) => {
    setLines((ls) => ls.map((l, j) => (j === i && l.kind === "row" ? { ...l, cells: l.cells.map((x, k) => (k === c ? v : x)) } : l)));
    setDirty(true);
  };
  const addRow = () => {
    const sec = section ?? sections[0] ?? "";
    const blank = { kind: "row" as const, cells: format.columns.map(() => ""), section: sec };
    if (format.id === "rules") blank.cells[0] = `LODGen${lines.filter((l) => l.kind === "row" && l.section === sec).length + 1}`;
    setLines((ls) => {
      // After the section's last row (or at the end).
      const last = ls.map((l, j) => (l.section === sec ? j : -1)).filter((j) => j >= 0).pop();
      const at = last === undefined ? ls.length : last + 1;
      return [...ls.slice(0, at), blank, ...ls.slice(at)];
    });
    setDirty(true);
  };
  const removeRow = (i: number) => {
    setLines((ls) => ls.filter((_, j) => j !== i));
    setDirty(true);
  };

  return (
    <Stack>
      <Group justify="space-between" align="flex-start">
        <div>
          <Title order={4}>{file.name.split("\\").pop()}</Title>
          <Text size="xs" c="dimmed">
            {file.source === "overlay" ? "Edited copy in the LODForge Config mod" : file.source === "missing" ? "Not present yet; saving creates it" : "DynDOLOD's file (unchanged)"} · {format?.label ?? "Text"}
          </Text>
        </div>
        <Group>
          {file.source === "overlay" && (
            <Button variant="default" leftSection={<IconRestore size={16} />} onClick={reset}>
              Reset to DynDOLOD's
            </Button>
          )}
          <Button leftSection={<IconDeviceFloppy size={16} />} disabled={!dirty} onClick={save}>
            Save
          </Button>
        </Group>
      </Group>
      {format && <Text size="sm">{format.description}</Text>}
      {format?.id === "rules" && <RulesChart lines={lines} />}
      {!format ? (
        <Textarea autosize minRows={20} styles={{ input: { fontFamily: "monospace", fontSize: 12 } }} value={text ?? ""} onChange={(e) => (setText(e.currentTarget.value), setDirty(true))} />
      ) : (
        <Card withBorder p={0}>
          <Group p="xs" gap="xs">
            <TextInput size="xs" leftSection={<IconSearch size={14} />} placeholder="Filter rows" value={query} onChange={(e) => setQuery(e.currentTarget.value)} style={{ flex: 1 }} />
            {format.sections && sections.length > 1 && <Select size="xs" placeholder="All sections" clearable data={sections} value={section} onChange={setSection} w={260} />}
            <Badge variant="light">{rows.length} rows</Badge>
            <Button size="xs" variant="light" leftSection={<IconPlus size={14} />} onClick={addRow}>
              Add row
            </Button>
          </Group>
          <ScrollArea h={460}>
            <Table stickyHeader striped highlightOnHover verticalSpacing={2}>
              <Table.Thead>
                <Table.Tr>
                  {format.sections && <Table.Th w={140}>Section</Table.Th>}
                  {format.columns.map((c) => (
                    <Table.Th key={c.label} w={c.width}>
                      {c.help ? (
                        <Tooltip label={c.help} withArrow>
                          <span style={{ textDecoration: "underline dotted" }}>{c.label}</span>
                        </Tooltip>
                      ) : (
                        c.label
                      )}
                    </Table.Th>
                  ))}
                  <Table.Th w={40} />
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {rows.slice(0, 500).map(({ l, i }) =>
                  l.kind === "row" ? (
                    <Table.Tr key={i}>
                      {format.sections && (
                        <Table.Td>
                          <Text size="xs" c="dimmed">
                            {l.section}
                          </Text>
                        </Table.Td>
                      )}
                      {format.columns.map((c, k) => (
                        <Table.Td key={k}>
                          {c.options ? (
                            <Select size="xs" data={c.options.map((o) => ({ value: o, label: o || "—" }))} value={l.cells[k] ?? ""} onChange={(v) => setCell(i, k, v ?? "")} allowDeselect={false} />
                          ) : (
                            <TextInput size="xs" variant="unstyled" value={l.cells[k] ?? ""} placeholder={c.placeholder} onChange={(e) => setCell(i, k, e.currentTarget.value)} spellCheck={false} />
                          )}
                        </Table.Td>
                      ))}
                      <Table.Td>
                        <ActionIcon variant="subtle" color="red" onClick={() => removeRow(i)} aria-label="Remove row">
                          <IconTrash size={14} />
                        </ActionIcon>
                      </Table.Td>
                    </Table.Tr>
                  ) : null,
                )}
              </Table.Tbody>
            </Table>
            {rows.length > 500 && (
              <Text size="xs" c="dimmed" p="xs">
                Showing the first 500 of {rows.length} rows; filter to find others.
              </Text>
            )}
          </ScrollArea>
        </Card>
      )}
    </Stack>
  );
}

/** What the rules of a preset give LOD4/8/16: how many rules show 3D, billboards, full models or nothing. */
function RulesChart({ lines }: { lines: Line[] }) {
  const rows = lines.filter((l): l is Extract<Line, { kind: "row" }> => l.kind === "row");
  const colors = { "3d": "teal.6", billboard: "yellow.6", full: "grape.6", none: "gray.6" } as const;
  const charts = [2, 3, 4].map((col, i) => {
    const counts: Record<string, number> = { "3d": 0, billboard: 0, full: 0, none: 0 };
    rows.forEach((r) => counts[levelKind(r.cells[col] ?? "")]++);
    return { label: ["LOD4", "LOD8", "LOD16"][i], data: Object.entries(counts).filter(([, n]) => n).map(([k, n]) => ({ name: k, value: n, color: colors[k as keyof typeof colors] })) };
  });
  return (
    <SimpleGrid cols={3}>
      {charts.map((c) => (
        <Card key={c.label} withBorder p="xs">
          <Text size="xs" fw={600} ta="center">
            {c.label}
          </Text>
          <Group justify="center">
            <DonutChart size={110} thickness={18} data={c.data} withTooltip tooltipDataSource="segment" />
          </Group>
        </Card>
      ))}
    </SimpleGrid>
  );
}
