<script lang="ts">
  import { tick } from "svelte";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import RotateCcwIcon from "@lucide/svelte/icons/rotate-ccw";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";
  import XIcon from "@lucide/svelte/icons/x";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";

  import {
    isTerminalSessionInteractive,
    MAX_TERMINAL_SESSIONS_PER_WINDOW,
    type TerminalState,
  } from "../../app/terminal-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";

  import TerminalSurface from "./TerminalSurface.svelte";

  let { state: terminalState }: { state: TerminalState } = $props();
  let pendingCloseSessionId = $state<string | null>(null);
  let pendingCloseAll = $state(false);
  let editingSessionId = $state<string | null>(null);
  let renameValue = $state("");
  let renameInput = $state<HTMLInputElement | null>(null);

  const requestClose = (sessionId: string) => {
    const session = terminalState.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    if (isTerminalSessionInteractive(session.state)) {
      pendingCloseSessionId = sessionId;
    } else {
      void terminalState.closeSession(sessionId);
    }
  };

  const confirmClose = () => {
    const sessionId = pendingCloseSessionId;
    pendingCloseSessionId = null;
    if (sessionId) void terminalState.closeSession(sessionId);
  };

  const beginRename = (sessionId: string) => {
    const session = terminalState.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    editingSessionId = sessionId;
    renameValue = session.title;
    void tick().then(() => {
      renameInput?.focus();
      renameInput?.select();
    });
  };

  const commitRename = () => {
    if (editingSessionId) {
      terminalState.renameSession(editingSessionId, renameValue);
    }
    editingSessionId = null;
    renameValue = "";
  };
</script>

<section
  class="flex h-full min-h-0 flex-col bg-background"
  aria-label="Integrated terminal"
>
  <header
    class="flex h-9 shrink-0 items-center gap-1 border-b bg-muted/20 px-2"
  >
    <div
      class="mr-1 flex min-w-0 items-center gap-1.5 px-1 text-xs font-medium"
    >
      <SquareTerminalIcon class="size-3.5 text-muted-foreground" />
      <span>Terminal</span>
    </div>

    <div
      class="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto"
      role="tablist"
      aria-label="Terminal sessions"
    >
      {#each terminalState.sessions as session (session.id)}
        <div
          class="group flex h-7 max-w-56 min-w-28 items-center rounded-md border border-transparent"
          class:border-border={terminalState.activeSessionId === session.id}
          class:bg-background={terminalState.activeSessionId === session.id}
        >
          {#if editingSessionId === session.id}
            <input
              bind:this={renameInput}
              class="mx-1 min-w-0 flex-1 rounded-sm border bg-background px-1.5 py-0.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
              aria-label={`Rename ${session.title} terminal`}
              maxlength="64"
              bind:value={renameValue}
              onblur={commitRename}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  commitRename();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  editingSessionId = null;
                }
              }}
            />
          {:else}
            <button
              type="button"
              role="tab"
              aria-selected={terminalState.activeSessionId === session.id}
              class="flex min-w-0 flex-1 items-center gap-1.5 px-2 text-left text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
              title={`${session.title} — ${session.contextLabel}`}
              onclick={() => terminalState.selectSession(session.id)}
              ondblclick={() => beginRename(session.id)}
            >
              <span
                class="size-1.5 shrink-0 rounded-full"
                class:bg-emerald-500={session.state === "running"}
                class:bg-amber-500={session.state === "starting" ||
                  session.state === "closing"}
                class:bg-muted-foreground={session.state === "exited"}
                class:bg-destructive={session.state === "failed"}
                aria-hidden="true"
              ></span>
              <span class="truncate">{session.title}</span>
            </button>
          {/if}
          <button
            type="button"
            class="mr-0.5 grid size-6 shrink-0 place-items-center rounded-sm text-muted-foreground opacity-70 outline-none group-hover:opacity-100 hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
            aria-label={`Close ${session.title} terminal`}
            onclick={() => requestClose(session.id)}
          >
            <XIcon class="size-3" />
          </button>
        </div>
      {/each}
    </div>

    {#if terminalState.activeSession?.state === "exited" || terminalState.activeSession?.state === "failed"}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Restart terminal"
        title="Restart terminal"
        onclick={() => {
          if (terminalState.activeSessionId)
            void terminalState.restartSession(terminalState.activeSessionId);
        }}
      >
        <RotateCcwIcon />
      </Button>
    {/if}
    <Button
      variant="ghost"
      size="icon-sm"
      disabled={terminalState.creating ||
        terminalState.sessions.length >= MAX_TERMINAL_SESSIONS_PER_WINDOW}
      aria-label="New terminal"
      title="New terminal"
      onclick={() => void terminalState.newTerminal()}
    >
      <PlusIcon />
    </Button>
    <DropdownMenu.Root>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Terminal actions"
            title="Terminal actions"
            {...props}
          >
            <MoreHorizontalIcon />
          </Button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="end">
        <DropdownMenu.Label>Terminal</DropdownMenu.Label>
        <DropdownMenu.Item
          disabled={terminalState.creating ||
            terminalState.sessions.length >= MAX_TERMINAL_SESSIONS_PER_WINDOW}
          onclick={() => void terminalState.newTerminal()}
        >
          New Terminal
          <DropdownMenu.Shortcut>Ctrl+Shift+`</DropdownMenu.Shortcut>
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={!terminalState.activeSession}
          onclick={() => terminalState.showAndFocus()}
        >
          Focus Terminal
        </DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item
          disabled={terminalState.sessions.length < 2}
          onclick={() => terminalState.selectRelativeSession(-1)}
        >
          Previous Terminal
          <DropdownMenu.Shortcut>Ctrl+PageUp</DropdownMenu.Shortcut>
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={terminalState.sessions.length < 2}
          onclick={() => terminalState.selectRelativeSession(1)}
        >
          Next Terminal
          <DropdownMenu.Shortcut>Ctrl+PageDown</DropdownMenu.Shortcut>
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={!terminalState.activeSessionId}
          onclick={() => {
            if (terminalState.activeSessionId)
              beginRename(terminalState.activeSessionId);
          }}
        >
          Rename Terminal
        </DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Label
          >Font size · {terminalState.fontSize}px</DropdownMenu.Label
        >
        <DropdownMenu.Item
          disabled={terminalState.fontSize <= 10}
          onclick={() => terminalState.setFontSize(terminalState.fontSize - 1)}
        >
          Decrease Font Size
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={terminalState.fontSize >= 24}
          onclick={() => terminalState.setFontSize(terminalState.fontSize + 1)}
        >
          Increase Font Size
        </DropdownMenu.Item>
        <DropdownMenu.Item
          onclick={() =>
            terminalState.setScrollback(
              terminalState.scrollback === 5_000 ? 10_000 : 5_000,
            )}
        >
          Scrollback · {terminalState.scrollback.toLocaleString()} lines
        </DropdownMenu.Item>
        <DropdownMenu.Item
          onclick={() =>
            terminalState.setScreenReaderMode(!terminalState.screenReaderMode)}
        >
          Screen reader mode
          <DropdownMenu.Shortcut>
            {terminalState.screenReaderMode ? "On" : "Off"}
          </DropdownMenu.Shortcut>
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={terminalState.activeSession?.state !== "exited" &&
            terminalState.activeSession?.state !== "failed"}
          onclick={() => {
            if (terminalState.activeSessionId)
              void terminalState.restartSession(terminalState.activeSessionId);
          }}
        >
          Restart Terminal
        </DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item
          disabled={!terminalState.activeSessionId}
          onclick={() => {
            if (terminalState.activeSessionId)
              requestClose(terminalState.activeSessionId);
          }}
        >
          Close Terminal
        </DropdownMenu.Item>
        <DropdownMenu.Item
          disabled={terminalState.sessions.length === 0}
          onclick={() => (pendingCloseAll = true)}
        >
          Close All Terminals
        </DropdownMenu.Item>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label="Hide terminal"
      title="Hide terminal (Ctrl+`)"
      onclick={() => terminalState.hide()}
    >
      <ChevronDownIcon />
    </Button>
  </header>

  <div class="relative min-h-0 flex-1">
    {#if terminalState.creating && terminalState.sessions.length === 0}
      <div
        class="grid h-full place-items-center text-xs text-muted-foreground"
        role="status"
      >
        Starting terminal…
      </div>
    {/if}
    {#if !terminalState.creating && terminalState.sessions.length === 0}
      <div class="grid h-full place-items-center p-6 text-center">
        <div>
          <SquareTerminalIcon
            class="mx-auto mb-2 size-5 text-muted-foreground"
          />
          <p class="text-sm font-medium">No terminal sessions</p>
          <p class="mt-1 text-xs text-muted-foreground">
            Start a shell in the current location.
          </p>
          <Button
            class="mt-3"
            size="sm"
            onclick={() => void terminalState.newTerminal()}
          >
            New terminal
          </Button>
        </div>
      </div>
    {/if}
    {#each terminalState.sessions as session (session.id)}
      <TerminalSurface
        state={terminalState}
        {session}
        active={terminalState.activeSessionId === session.id}
      />
    {/each}

    {#if terminalState.activeSession?.state === "exited"}
      <div
        class="pointer-events-none absolute right-3 bottom-3 rounded-md border bg-background/95 px-2 py-1 text-[11px] text-muted-foreground shadow-sm"
      >
        Process exited{terminalState.activeSession.exitCode === null
          ? ""
          : ` · code ${terminalState.activeSession.exitCode}`}
      </div>
    {:else if terminalState.activeSession?.state === "failed"}
      <div
        class="absolute inset-x-3 bottom-3 rounded-md border border-destructive/30 bg-background/95 px-3 py-2 text-xs text-destructive shadow-sm"
        role="alert"
      >
        {terminalState.activeSession.errorMessage ??
          "The terminal session failed."}
      </div>
    {/if}
  </div>

  <p class="sr-only" aria-live="polite">
    {terminalState.statusAnnouncement}
  </p>
</section>

<Dialog.Root
  open={terminalState.pendingPaste !== null}
  onOpenChange={(open) => {
    if (!open) terminalState.cancelPaste();
  }}
>
  <Dialog.Content showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>Paste multiple lines?</Dialog.Title>
      <Dialog.Description>
        This will send {terminalState.pendingPaste?.lineCount ?? 0} lines directly
        to {terminalState.pendingPaste?.targetLabel ?? "the terminal"}.
      </Dialog.Description>
    </Dialog.Header>
    <pre
      class="max-h-44 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap">{terminalState
        .pendingPaste?.preview ?? ""}</pre>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => terminalState.cancelPaste()}>
        Cancel
      </Button>
      <Button onclick={() => terminalState.confirmPaste()}>Paste</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root
  open={pendingCloseAll}
  onOpenChange={(open) => {
    if (!open) pendingCloseAll = false;
  }}
>
  <Dialog.Content showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>Close all terminals?</Dialog.Title>
      <Dialog.Description>
        This will close {terminalState.sessions.length}
        {terminalState.sessions.length === 1 ? "session" : "sessions"} and terminate
        any commands still running.
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (pendingCloseAll = false)}>
        Keep open
      </Button>
      <Button
        variant="destructive"
        onclick={() => {
          pendingCloseAll = false;
          terminalState.closeAll();
        }}
      >
        Close all terminals
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root
  open={pendingCloseSessionId !== null}
  onOpenChange={(open) => {
    if (!open) pendingCloseSessionId = null;
  }}
>
  <Dialog.Content showCloseButton={false}>
    {@const closingSession = terminalState.sessions.find(
      ({ id }) => id === pendingCloseSessionId,
    )}
    <Dialog.Header>
      <Dialog.Title>Close this terminal?</Dialog.Title>
      <Dialog.Description>
        Any command still running in {closingSession?.contextLabel ??
          "this terminal"} will be terminated.
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (pendingCloseSessionId = null)}>
        Keep open
      </Button>
      <Button variant="destructive" onclick={confirmClose}>
        Close terminal
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
