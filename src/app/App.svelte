<script lang="ts">
  import { onMount } from "svelte";
  import { isTauri } from "@tauri-apps/api/core";
  import { ModeWatcher } from "mode-watcher";

  import ExplorerShell from "../features/explorer/ExplorerShell.svelte";
  import { Toaster } from "$lib/components/ui/sonner";
  import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
  import { TauriExplorerDataSource } from "$lib/data/tauri-explorer-data-source";

  import { ExplorerState } from "./explorer-state.svelte";

  const state = new ExplorerState(
    isTauri() ? new TauriExplorerDataSource() : new DemoExplorerDataSource(),
  );

  onMount(() => {
    void state.initialize();
  });
</script>

<ModeWatcher />
<Toaster />
<ExplorerShell {state} />
