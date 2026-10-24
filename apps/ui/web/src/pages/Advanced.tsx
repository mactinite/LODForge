import { useState } from "react";
import { Accordion, Badge, Button, Card, Code, CopyButton, Group, SimpleGrid, Stack, Switch, Text, TextInput } from "@mantine/core";
import { IconCopy, IconRestore, IconSearch } from "@tabler/icons-react";

import { OptionControl } from "../components/OptionControl";
import { DEFAULTS, GROUPS, OPTIONS, Values } from "../options";

/** Every build option by area, with search, expert toggle and the resulting command line. */
export function Advanced({ values, setValues, command }: { values: Values; setValues: (v: Values) => void; command: string[] }) {
  const [query, setQuery] = useState("");
  const [expert, setExpert] = useState(false);
  const q = query.trim().toLowerCase();
  const changed = OPTIONS.filter((o) => values[o.key] !== undefined && values[o.key] !== o.default).length;
  const visible = (o: (typeof OPTIONS)[number]) =>
    (expert || !o.expert || values[o.key] !== o.default) && (!o.when || o.when(values)) && (!q || `${o.label} ${o.flag} ${o.explain}`.toLowerCase().includes(q));
  const set = (key: string, v: string | number | boolean) => setValues({ ...values, [key]: v });

  return (
    <Stack>
      <Group justify="space-between">
        <Group>
          <TextInput leftSection={<IconSearch size={16} />} placeholder="Search options" value={query} onChange={(e) => setQuery(e.currentTarget.value)} w={280} />
          <Switch label="Show expert options" checked={expert} onChange={(e) => setExpert(e.currentTarget.checked)} />
        </Group>
        <Group>
          <Badge variant="light">{changed} changed from defaults</Badge>
          <Button variant="default" leftSection={<IconRestore size={16} />} onClick={() => setValues({ ...DEFAULTS })}>
            Reset all
          </Button>
        </Group>
      </Group>
      <Accordion multiple defaultValue={["stages", "objects", "trees"]} variant="separated">
        {GROUPS.map((g) => {
          const opts = OPTIONS.filter((o) => o.group === g.id && visible(o));
          if (!opts.length) return null;
          return (
            <Accordion.Item key={g.id} value={g.id}>
              <Accordion.Control>
                <Group gap="xs">
                  <Text fw={600}>{g.label}</Text>
                  <Text size="xs" c="dimmed">
                    {g.description}
                  </Text>
                </Group>
              </Accordion.Control>
              <Accordion.Panel>
                <SimpleGrid cols={{ base: 1, md: 2 }} spacing="lg" verticalSpacing="lg">
                  {opts.map((o) => (
                    <OptionControl key={o.key} option={o} values={values} onChange={set} />
                  ))}
                </SimpleGrid>
              </Accordion.Panel>
            </Accordion.Item>
          );
        })}
      </Accordion>
      <Card withBorder>
        <Group justify="space-between" mb="xs">
          <Text fw={600} size="sm">
            Command line
          </Text>
          <CopyButton value={command.map((a) => (/[\s"]/.test(a) ? `"${a}"` : a)).join(" ")}>
            {({ copied, copy }) => (
              <Button size="xs" variant="light" leftSection={<IconCopy size={14} />} onClick={copy}>
                {copied ? "Copied" : "Copy"}
              </Button>
            )}
          </CopyButton>
        </Group>
        <Code block>{"lodforge " + command.map((a) => (/[\s"]/.test(a) ? `"${a}"` : a)).join(" ")}</Code>
      </Card>
    </Stack>
  );
}
