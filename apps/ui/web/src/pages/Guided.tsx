import { useEffect, useState } from "react";
import { Alert, Badge, Button, Card, Checkbox, Chip, Group, Radio, Select, SimpleGrid, Stack, Stepper, Switch, Text, TextInput, Title } from "@mantine/core";
import { IconFolder, IconInfoCircle, IconRefresh } from "@tabler/icons-react";

import { ProfileInfo, invoke } from "../backend";
import { Features } from "../components/Features";
import { ImpactCard } from "../components/Impact";
import { OptionControl } from "../components/OptionControl";
import { LEVELS, Values, isRuntime, levelOf, optionByKey } from "../options";
import { Output, Target } from "../state";

const SEASONS: Record<string, string> = { SPR: "Spring", SUM: "Summer", AUT: "Autumn", WIN: "Winter" };

interface Props {
  target: Target;
  setTarget: (t: Target) => void;
  info: ProfileInfo | null;
  setInfo: (i: ProfileInfo | null) => void;
  values: Values;
  setValues: (v: Values) => void;
  output: Output;
  setOutput: (o: Output) => void;
  onBuild: () => void;
  building: boolean;
  step: number;
  setStep: (n: number) => void;
}

/** The guided flow: instance, worldspaces, quality, output, build. */
export function Guided(p: Props) {
  const { target, info, values, output } = p;
  const [profiles, setProfiles] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState("");

  const loadProfiles = async (instance: string) => {
    if (!instance.trim()) return setProfiles([]);
    try {
      const list = await invoke<string[]>("list_profiles", { instance });
      setProfiles(list);
      if (!list.includes(target.profile) && list.length) p.setTarget({ ...target, instance, profile: list.includes("Default") ? "Default" : list[0] });
      setError("");
    } catch (e) {
      setProfiles([]);
      setError(String(e));
    }
  };
  useEffect(() => {
    loadProfiles(target.instance);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target.instance]);

  const scan = async () => {
    setScanning(true);
    try {
      const i = await invoke<ProfileInfo>("profile_info", { instance: target.instance, profile: target.profile });
      p.setInfo(i);
      p.setTarget({ ...target, worlds: target.worlds.filter((w) => i.worlds.includes(w)).length ? target.worlds.filter((w) => i.worlds.includes(w)) : i.worlds, seasons: target.seasons.filter((s) => i.seasons.includes(s)) });
      setError("");
    } catch (e) {
      p.setInfo(null);
      setError(String(e));
    }
    setScanning(false);
  };

  const level = levelOf(values);
  const set = (key: string, v: string | number | boolean) => p.setValues({ ...values, [key]: v });
  const control = (key: string) => {
    const o = optionByKey(key);
    return o && (!o.when || o.when(values)) ? <OptionControl key={key} option={o} values={values} onChange={set} /> : null;
  };
  const canNext = [!!info, target.worlds.length > 0, true, output.dest === "mo2" ? !!output.modName.trim() : !!output.folder.trim()];

  return (
    <Stack>
      <Stepper active={p.step} onStepClick={(s) => (s <= p.step || canNext.slice(0, s).every(Boolean)) && p.setStep(s)} allowNextStepsSelect={false} size="sm">
        <Stepper.Step label="Instance" description="MO2 folder and profile">
          <Stack mt="md" maw={720}>
            <Title order={3}>Which Mod Organizer instance?</Title>
            <Text c="dimmed" size="sm">
              The portable MO2 folder with ModOrganizer.exe, mods\ and profiles\. The game is taken from its Stock Game folder.
            </Text>
            <TextInput label="Instance folder" leftSection={<IconFolder size={16} />} placeholder="D:\Modlists\skyrim-cs-base" value={target.instance} onChange={(e) => p.setTarget({ ...target, instance: e.currentTarget.value })} spellCheck={false} />
            <Group align="end">
              <Select label="Profile" data={profiles} value={target.profile} onChange={(x) => x && p.setTarget({ ...target, profile: x })} w={240} />
              <Button leftSection={<IconRefresh size={16} />} loading={scanning} onClick={scan} disabled={!target.instance || !target.profile}>
                Scan load order
              </Button>
            </Group>
            {error && <Alert color="red">{error}</Alert>}
            {info && (
              <Card withBorder>
                <Group gap="xl">
                  <Stat label="Plugins" value={info.plugins} />
                  <Stat label="LOD worldspaces" value={info.worlds.length} />
                  <Stat label="Seasons" value={info.seasons.length || "none"} />
                  <Stat label="LODForge Runtime" value={info.runtime_plugin ? "installed" : "not installed"} />
                  <Stat label="Scan" value={`${info.ms} ms`} />
                </Group>
                {info.missing_archives > 0 && (
                  <Text c="yellow" size="sm" mt="xs">
                    {info.missing_archives} archive(s) listed by plugins were not found.
                  </Text>
                )}
              </Card>
            )}
          </Stack>
        </Stepper.Step>

        <Stepper.Step label="Worldspaces" description="What to build">
          <Stack mt="md">
            <Title order={3}>Which worldspaces?</Title>
            <Text c="dimmed" size="sm">
              Only worldspaces with a lodsettings file can have LOD. Tamriel is the big one; the rest take seconds.
            </Text>
            <Group>
              <Button variant="subtle" size="xs" onClick={() => p.setTarget({ ...target, worlds: info?.worlds ?? [] })}>
                Select all
              </Button>
              <Button variant="subtle" size="xs" onClick={() => p.setTarget({ ...target, worlds: [] })}>
                None
              </Button>
            </Group>
            <Checkbox.Group value={target.worlds} onChange={(w) => p.setTarget({ ...target, worlds: w })}>
              <SimpleGrid cols={{ base: 2, md: 4 }}>
                {(info?.worlds ?? []).map((w) => (
                  <Checkbox.Card key={w} value={w} p="sm" radius="md">
                    <Group wrap="nowrap">
                      <Checkbox.Indicator />
                      <Text size="sm">{w}</Text>
                      {w === "Tamriel" && <Badge size="xs" variant="light">largest</Badge>}
                    </Group>
                  </Checkbox.Card>
                ))}
              </SimpleGrid>
            </Checkbox.Group>
            <Title order={5} mt="sm">
              Seasons
            </Title>
            {info?.seasons.length ? (
              <Chip.Group multiple value={target.seasons} onChange={(s) => p.setTarget({ ...target, seasons: s })}>
                <Group>
                  {info.seasons.map((s) => (
                    <Chip key={s} value={s}>
                      {SEASONS[s] ?? s}
                    </Chip>
                  ))}
                </Group>
              </Chip.Group>
            ) : (
              <Text size="sm" c="dimmed">
                No season swap files found, so only the default season can be built.
              </Text>
            )}
            <Text size="xs" c="dimmed">
              The default season is always built; each extra season adds its own terrain textures and object LOD.
            </Text>
          </Stack>
        </Stepper.Step>

        <Stepper.Step label="Quality" description="Level and key choices">
          <Stack mt="md">
            <Title order={3}>How much detail?</Title>
            <Radio.Group value={level} onChange={(l) => l !== "custom" && p.setValues({ ...values, ...LEVELS[l].values })}>
              <SimpleGrid cols={{ base: 2, md: 4 }}>
                {[...Object.entries(LEVELS), ["custom", { label: "Custom", description: "Your own mix; change anything here or in the Advanced tab." }] as const].map(([id, l]) => (
                  <Radio.Card key={id} value={id} p="md" radius="md">
                    <Group wrap="nowrap" align="flex-start">
                      <Radio.Indicator />
                      <div>
                        <Text fw={600}>{l.label}</Text>
                        <Text size="xs" c="dimmed">
                          {isRuntime(values) && "runtime" in l && l.runtime ? l.runtime : l.description}
                        </Text>
                      </div>
                    </Group>
                  </Radio.Card>
                ))}
              </SimpleGrid>
            </Radio.Group>
            <SimpleGrid cols={{ base: 1, md: 2 }} spacing="lg">
              <Stack>
                {control("preset")}
                {control("treeLod")}
                {control("treeModels")}
                {control("materials")}
              </Stack>
              <Stack>
                <ImpactCard values={values} />
                {control("bc7")}
                {control("atlasMaxTile")}
              </Stack>
            </SimpleGrid>
            <Title order={4} mt="sm">
              What the build adds
            </Title>
            <Features info={info} values={values} set={set} setMany={p.setValues} control={control} />
            <Alert variant="light" icon={<IconInfoCircle />}>
              Every other option (terrain, billboards, input and expert settings) is in the Advanced tab; config files such as the child worldspace lists are under Config files.
            </Alert>
          </Stack>
        </Stepper.Step>

        <Stepper.Step label="Output" description="Where it goes">
          <Stack mt="md" maw={720}>
            <Title order={3}>Where should the output go?</Title>
            <Radio.Group value={output.dest} onChange={(d) => p.setOutput({ ...output, dest: d as Output["dest"] })}>
              <Stack>
                <Radio value="mo2" label="Into an MO2 mod" />
                {output.dest === "mo2" && (
                  <Stack pl="xl" gap="xs">
                    <TextInput label="Mod name" value={output.modName} onChange={(e) => p.setOutput({ ...output, modName: e.currentTarget.value })} />
                    <Switch label="Enable it in the profile (MO2 must be closed)" checked={output.enable} onChange={(e) => p.setOutput({ ...output, enable: e.currentTarget.checked })} />
                  </Stack>
                )}
                <Radio value="folder" label="Into a folder" />
                {output.dest === "folder" && <TextInput pl="xl" label="Folder" placeholder="D:\LOD\Data" value={output.folder} onChange={(e) => p.setOutput({ ...output, folder: e.currentTarget.value })} />}
              </Stack>
            </Radio.Group>
          </Stack>
        </Stepper.Step>
      </Stepper>

      <Group justify="flex-end" mt="md">
        <Button variant="default" disabled={p.step === 0} onClick={() => p.setStep(p.step - 1)}>
          Back
        </Button>
        {p.step < 3 ? (
          <Button disabled={!canNext[p.step]} onClick={() => p.setStep(p.step + 1)}>
            Next
          </Button>
        ) : (
          <Button color="green" disabled={!canNext.every(Boolean) || p.building} onClick={p.onBuild}>
            Start build
          </Button>
        )}
      </Group>
    </Stack>
  );
}

function Stat({ label, value }: { label: string; value: string | number }) {
  return (
    <div>
      <Text size="xs" c="dimmed">
        {label}
      </Text>
      <Text fw={700} size="lg">
        {value}
      </Text>
    </div>
  );
}
