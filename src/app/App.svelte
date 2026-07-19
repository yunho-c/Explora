<script lang="ts">
  import { onMount } from "svelte";
  import { ModeWatcher } from "mode-watcher";

  import ExplorerShell from "../features/explorer/ExplorerShell.svelte";
  import { Toaster } from "$lib/components/ui/sonner";
  import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";

  import { ExplorerState } from "./explorer-state.svelte";

  const state = new ExplorerState(new DemoExplorerDataSource());

  onMount(() => {
    void state.initialize();
  });
</script>

<ModeWatcher />
<Toaster />
<ExplorerShell {state} />
