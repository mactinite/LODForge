import { useEffect, useMemo, useState } from "react";
import { ActionIcon, AppShell, Badge, Button, Group, Image, Modal, Stack, Tabs, Text, Tooltip, useMantineColorScheme } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { IconAdjustments, IconChartBar, IconEye, IconFileSettings, IconMoon, IconRoute, IconSun } from "@tabler/icons-react";

import { ProfileInfo, invoke, isTauri } from "./backend";
import { BuildPanel } from "./components/BuildPanel";
import { DEFAULTS, LEVELS, Values, levelOf } from "./options";
import { Advanced } from "./pages/Advanced";
import { ConfigFiles } from "./pages/ConfigFiles";
import { Guided } from "./pages/Guided";
import { Reports } from "./pages/Reports";
import { Output, Target, buildArgs, useBuild, useStored } from "./state";

export function App() {
  const [tab, setTab] = useState<string>("guided");
  const [target, setTarget] = useStored<Target>("target", { instance: "", profile: "Default", worlds: [], seasons: [] });
  const [values, setValues] = useStored<Values>("values", { ...DEFAULTS, ...LEVELS.balanced.values });
  const [output, setOutput] = useStored<Output>("output", { dest: "mo2", modName: "LODForge Output", folder: "", enable: false });
  const [info, setInfo] = useState<ProfileInfo | null>(null);
  const [step, setStep] = useState(0);
  const [showBuild, setShowBuild] = useState(false);
  const build = useBuild();
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();

  // New options added after a profile was saved get their defaults.
  useEffect(() => setValues((v) => ({ ...DEFAULTS, ...v })), [setValues]);

  // `lodforge-ui --instance <folder> [--profile <name>]` wins over the remembered instance.
  useEffect(() => {
    invoke<{ instance?: string | null; profile?: string | null }>("launch_args").then((a) => {
      if (a?.instance) setTarget((t) => ({ ...t, instance: a.instance!, profile: a.profile ?? t.profile }));
    });
  }, [setTarget]);

  const command = useMemo(() => buildArgs(target, values, output), [target, values, output]);
  // Rough stage count for the progress bar.
  const expected = useMemo(() => {
    const perWorld = (values.terrain ? 3 + target.seasons.length : 0) + (values.objects ? 4 : 0) + (values.trees ? 1 : 0);
    const once = (values.texgen ? 2 : 0) + (values.objects ? 2 : 0) + (values.plugins ? 1 : 0) + (values.occlusion ? 1 : 0) + (values.treeLod === "ultra" && values.treeModels !== "off" ? 1 : 0);
    return Math.max(1, perWorld * Math.max(1, target.worlds.length) + once);
  }, [values, target]);

  const startBuild = async () => {
    if (output.enable && (await invoke<boolean>("mo2_running"))) {
      notifications.show({ color: "red", title: "Mod Organizer is open", message: "Close it before enabling the mod, or untick “Enable”." });
      return;
    }
    try {
      await build.start(command.slice(1));
      setShowBuild(true);
    } catch (e) {
      notifications.show({ color: "red", title: "Build did not start", message: String(e) });
    }
  };

  const level = levelOf(values);
  return (
    <AppShell header={{ height: 64 }} padding="lg">
      <AppShell.Header>
        <Group h="100%" px="lg" justify="space-between" wrap="nowrap">
          <Group gap="sm" wrap="nowrap">
            <Image src="icon.png" w={40} h={40} />
            <div>
              <Text fw={700} size="lg" lh={1.1}>
                LODForge
              </Text>
              <Text size="xs" c="dimmed">
                distant LOD for Skyrim SE
              </Text>
            </div>
          </Group>
          <Tabs value={tab} onChange={(t) => t && setTab(t)} variant="pills">
            <Tabs.List>
              <Tabs.Tab value="guided" leftSection={<IconRoute size={16} />}>
                Guided
              </Tabs.Tab>
              <Tabs.Tab value="advanced" leftSection={<IconAdjustments size={16} />}>
                Advanced
              </Tabs.Tab>
              <Tabs.Tab value="config" leftSection={<IconFileSettings size={16} />}>
                Config files
              </Tabs.Tab>
              <Tabs.Tab value="reports" leftSection={<IconChartBar size={16} />}>
                Reports
              </Tabs.Tab>
            </Tabs.List>
          </Tabs>
          <Group gap="xs" wrap="nowrap">
            {!isTauri && (
              <Badge color="gray" variant="light">
                preview (mock backend)
              </Badge>
            )}
            <Badge variant="light">{level === "custom" ? "Custom" : LEVELS[level].label}</Badge>
            {build.state.startedAt > 0 && (
              <Button size="xs" variant={build.state.running ? "filled" : "light"} leftSection={<IconEye size={14} />} onClick={() => setShowBuild(true)}>
                {build.state.running ? "Building…" : "Last build"}
              </Button>
            )}
            <Tooltip label="Light / dark">
              <ActionIcon variant="default" onClick={toggleColorScheme} aria-label="Toggle colour scheme">
                {colorScheme === "dark" ? <IconSun size={16} /> : <IconMoon size={16} />}
              </ActionIcon>
            </Tooltip>
          </Group>
        </Group>
      </AppShell.Header>
      <AppShell.Main>
        {tab === "guided" && (
          <Guided target={target} setTarget={setTarget} info={info} setInfo={setInfo} values={values} setValues={setValues} output={output} setOutput={setOutput} onBuild={startBuild} building={build.state.running} step={step} setStep={setStep} />
        )}
        {tab === "advanced" && (
          <Stack>
            <Advanced values={values} setValues={setValues} command={command} />
            <Group justify="flex-end">
              <Button color="green" disabled={!info || !target.worlds.length || build.state.running} onClick={startBuild}>
                Start build
              </Button>
            </Group>
          </Stack>
        )}
        {tab === "config" && <ConfigFiles ready={!!info} />}
        {tab === "reports" && <Reports worlds={info?.worlds ?? target.worlds} output={output} instance={target.instance} />}
      </AppShell.Main>
      <Modal opened={showBuild} onClose={() => setShowBuild(false)} size="xl" title="Build" centered>
        <BuildPanel state={build.state} expected={expected} onCancel={build.cancel} />
        {!build.state.running && build.state.exitCode !== null && (
          <Group justify="flex-end" mt="md">
            <Button variant="default" onClick={() => (setShowBuild(false), setTab("reports"))}>
              Tree report
            </Button>
            <Button
              onClick={() =>
                invoke("open_viewer", { req: { instance: target.instance, profile: target.profile, world: target.worlds[0] ?? "Tamriel", out: output.dest === "folder" ? output.folder : null, mo2_mod: output.dest === "mo2" ? output.modName : null } }).catch((e) =>
                  notifications.show({ color: "red", title: "Viewer", message: String(e) }),
                )
              }
            >
              Inspect LODs
            </Button>
          </Group>
        )}
      </Modal>
    </AppShell>
  );
}
