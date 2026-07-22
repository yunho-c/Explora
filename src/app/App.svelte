<script lang="ts">
  import { onMount } from "svelte";
  import { isTauri } from "@tauri-apps/api/core";
  import { ModeWatcher } from "mode-watcher";
  import { toast } from "svelte-sonner";

  import ExplorerShell from "../features/explorer/ExplorerShell.svelte";
  import { Toaster } from "$lib/components/ui/sonner";
  import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
  import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
  import { TauriExplorerDataSource } from "$lib/data/tauri-explorer-data-source";
  import { TauriPreferencesDataSource } from "$lib/data/tauri-preferences-data-source";

  import { ExplorerState } from "./explorer-state.svelte";
  import { WindowChromeController } from "./window-chrome.svelte";

  const runningInTauri = isTauri();
  const state = new ExplorerState(
    runningInTauri
      ? new TauriExplorerDataSource()
      : new DemoExplorerDataSource(),
    runningInTauri
      ? new TauriPreferencesDataSource()
      : new MemoryPreferencesDataSource(),
  );
  const windowChrome = new WindowChromeController();
  let shownRecoveryMessage: string | null = null;
  let shownPreferencesWarning: string | null = null;
  let shownVolumeWarning: string | null = null;

  $effect(() => {
    const message = windowChrome.recoveryMessage;
    if (message && message !== shownRecoveryMessage) {
      shownRecoveryMessage = message;
      toast.warning("Window controls recovered with limited behavior", {
        description: message,
      });
    }
  });

  $effect(() => {
    const message = state.volumeWarningMessage;
    if (message && message !== shownVolumeWarning) {
      shownVolumeWarning = message;
      toast.warning("Mounted volumes may be out of date", {
        description: message,
      });
    }
  });

  $effect(() => {
    const message = state.preferencesWarningMessage;
    if (message && message !== shownPreferencesWarning) {
      shownPreferencesWarning = message;
      toast.warning("Preferences could not be fully restored", {
        description: message,
      });
    }
  });

  onMount(() => {
    let disposed = false;
    let stopWindowChrome = () => {};

    void (async () => {
      await state.initializePreferences();
      if (disposed) return;
      stopWindowChrome = windowChrome.start();
      await state.initialize();
    })();

    return () => {
      disposed = true;
      state.dispose();
      stopWindowChrome();
    };
  });
</script>

<ModeWatcher />
<Toaster />
<ExplorerShell {state} {windowChrome} />
