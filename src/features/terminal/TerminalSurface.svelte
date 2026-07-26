<script lang="ts">
  import { onMount } from "svelte";

  import type {
    TerminalSessionView,
    TerminalState,
  } from "../../app/terminal-state.svelte";

  import { XtermAdapter } from "./xterm-adapter";

  let {
    state,
    session,
    active,
  }: {
    state: TerminalState;
    session: TerminalSessionView;
    active: boolean;
  } = $props();

  let mount: HTMLDivElement;
  let adapter: XtermAdapter | null = null;

  onMount(() => {
    adapter = new XtermAdapter(
      mount,
      {
        onData: (value) => state.sendText(session.id, value),
        onBinary: (value) => state.sendBinaryString(session.id, value),
        onResize: (size) => void state.resize(session.id, size),
        onPaste: (text) => state.requestPaste(session.id, text),
        onToggleVisibility: () => state.toggleVisibility(),
        onNewTerminal: () => void state.newTerminal(),
        onNextSession: () => state.selectRelativeSession(1),
        onPreviousSession: () => state.selectRelativeSession(-1),
      },
      {
        fontSize: state.fontSize,
        scrollback: state.scrollback,
        screenReaderMode: state.screenReaderMode,
      },
    );
    const unsubscribe = state.subscribeOutput(session.id, (event) => {
      adapter?.write(event.bytes, () =>
        state.acknowledgeOutput(session.id, event.sequence),
      );
    });
    if (active) adapter.focus();

    return () => {
      unsubscribe();
      adapter?.dispose();
      adapter = null;
    };
  });

  $effect(() => {
    const focusRequest = state.focusRequest;
    if (active && focusRequest >= 0) {
      requestAnimationFrame(() => {
        adapter?.fit();
        adapter?.focus();
      });
    }
  });

  $effect(() => {
    adapter?.setPreferences({
      fontSize: state.fontSize,
      scrollback: state.scrollback,
      screenReaderMode: state.screenReaderMode,
    });
  });
</script>

<div
  bind:this={mount}
  class:hidden={!active}
  class="explora-terminal-mount h-full min-h-0 w-full overflow-hidden"
  data-terminal-surface
  aria-label={`${session.title} terminal`}
></div>

<style>
  .explora-terminal-mount {
    --terminal-background: #111318;
    --terminal-foreground: #eef1f5;
    --terminal-cursor: #f8fafc;
    --terminal-cursor-accent: #111318;
    --terminal-selection-background: #34435a;
    --terminal-selection-foreground: #ffffff;
    --terminal-focus-ring: #7dd3fc;
    --terminal-scrollbar-thumb: #4a5361;

    background-color: var(--terminal-background);
  }

  :global(.explora-terminal-mount .xterm) {
    box-sizing: border-box;
    height: 100%;
    padding: 0.5rem 0.625rem;
    background-color: var(--terminal-background);
  }

  :global(.explora-terminal-mount .xterm-viewport) {
    /* xterm writes its default black viewport background inline. Keep the
       scroll gutter and padded renderer on the same intentional surface. */
    background-color: var(--terminal-background) !important;
    scrollbar-width: thin;
    scrollbar-color: var(--terminal-scrollbar-thumb) var(--terminal-background);
  }

  :global(.explora-terminal-mount .xterm:focus-within) {
    outline: 1px solid
      color-mix(in oklch, var(--terminal-focus-ring) 72%, transparent);
    outline-offset: -1px;
  }

  @media (forced-colors: active) {
    :global(.explora-terminal-mount .xterm:focus-within) {
      outline: 2px solid Highlight;
    }
  }
</style>
