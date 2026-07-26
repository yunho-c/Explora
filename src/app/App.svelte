<script lang="ts">
  import { onMount } from "svelte";
  import { isTauri } from "@tauri-apps/api/core";
  import { ModeWatcher } from "mode-watcher";
  import { toast } from "svelte-sonner";

  import ExplorerShell from "../features/explorer/ExplorerShell.svelte";
  import { Toaster } from "$lib/components/ui/sonner";
  import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
  import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
  import { DemoTerminalDataSource } from "$lib/data/demo-terminal-data-source";
  import { TauriExplorerDataSource } from "$lib/data/tauri-explorer-data-source";
  import { TauriPreferencesDataSource } from "$lib/data/tauri-preferences-data-source";
  import { TauriTerminalDataSource } from "$lib/data/tauri-terminal-data-source";

  import { ExplorerState } from "./explorer-state.svelte";
  import { TerminalState } from "./terminal-state.svelte";
  import { WindowChromeController } from "./window-chrome.svelte";

  const runningInTauri = isTauri();
  const preferencesDataSource = runningInTauri
    ? new TauriPreferencesDataSource()
    : new MemoryPreferencesDataSource();
  const state = new ExplorerState(
    runningInTauri
      ? new TauriExplorerDataSource()
      : new DemoExplorerDataSource(),
    preferencesDataSource,
  );
  const terminalState = new TerminalState(
    runningInTauri
      ? new TauriTerminalDataSource()
      : new DemoTerminalDataSource(),
    () => {
      const location = state.activeLocation;
      const directory = state.activeDirectory;
      if (!location || !directory) return null;
      return {
        locationId: location.id,
        directoryId: location.kind === "ssh" ? null : directory.id,
        kind: location.kind === "ssh" ? "ssh" : "local",
        locationLabel: location.name,
        directoryLabel:
          location.kind === "ssh"
            ? `${location.name} · server home`
            : directory.displayPath,
      };
    },
    preferencesDataSource,
  );
  const windowChrome = new WindowChromeController();
  let shownRecoveryMessage: string | null = null;
  let shownPreferencesWarning: string | null = null;
  let shownVolumeWarning: string | null = null;
  let shownNativeOpenWarning: string | null = null;
  let shownNativeOpenError: string | null = null;
  let shownTerminalError: string | null = null;
  let shownTerminalPreferencesWarning: string | null = null;

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
    const message = state.nativeOpenWarningMessage;
    if (message && message !== shownNativeOpenWarning) {
      shownNativeOpenWarning = message;
      toast.warning("Temporary files need attention", { description: message });
    }
  });

  $effect(() => {
    const message = state.nativeOpenErrorMessage;
    if (message && message !== shownNativeOpenError) {
      shownNativeOpenError = message;
      toast.error("Could not open item", { description: message });
    }
  });

  $effect(() => {
    const message = terminalState.preferencesWarningMessage;
    if (message && message !== shownTerminalPreferencesWarning) {
      shownTerminalPreferencesWarning = message;
      toast.warning("Terminal preferences could not be fully restored", {
        description: message,
      });
    }
  });

  $effect(() => {
    const message = terminalState.errorMessage;
    if (message && message !== shownTerminalError) {
      shownTerminalError = message;
      toast.error("Terminal operation failed", { description: message });
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
      await Promise.all([
        state.initializePreferences(),
        terminalState.initializePreferences(),
      ]);
      if (disposed) return;
      stopWindowChrome = windowChrome.start();
      await state.initialize();
    })();

    return () => {
      disposed = true;
      terminalState.dispose();
      state.dispose();
      stopWindowChrome();
    };
  });
</script>

<ModeWatcher />
<Toaster />
<ExplorerShell {state} {terminalState} {windowChrome} />
