<script lang="ts">
  import { onMount } from "svelte";
  import { isTauri } from "@tauri-apps/api/core";
  import { ModeWatcher } from "mode-watcher";
  import { toast } from "svelte-sonner";

  import ExplorerShell from "../features/explorer/ExplorerShell.svelte";
  import { Toaster } from "$lib/components/ui/sonner";
  import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
  import { TauriExplorerDataSource } from "$lib/data/tauri-explorer-data-source";

  import { ExplorerState } from "./explorer-state.svelte";
  import { WindowChromeController } from "./window-chrome.svelte";

  const state = new ExplorerState(
    isTauri() ? new TauriExplorerDataSource() : new DemoExplorerDataSource(),
  );
  const windowChrome = new WindowChromeController();
  let shownRecoveryMessage: string | null = null;

  $effect(() => {
    const message = windowChrome.recoveryMessage;
    if (message && message !== shownRecoveryMessage) {
      shownRecoveryMessage = message;
      toast.warning("Window controls recovered with limited behavior", {
        description: message,
      });
    }
  });

  onMount(() => {
    const stopWindowChrome = windowChrome.start();
    void state.initialize();
    return stopWindowChrome;
  });
</script>

<ModeWatcher />
<Toaster />
<ExplorerShell {state} {windowChrome} />
