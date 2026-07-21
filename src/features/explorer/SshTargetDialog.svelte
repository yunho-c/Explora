<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Switch } from "$lib/components/ui/switch";
  import type { ExplorerState } from "../../app/explorer-state.svelte";

  let { state: explorerState }: { state: ExplorerState } = $props();

  let loadedKey = "";
  let name = $state("");
  let host = $state("");
  let port = $state(22);
  let username = $state("");
  let initialPath = $state("");
  let identityFile = $state("");
  let identitiesOnly = $state(false);

  $effect(() => {
    if (!explorerState.sshTargetDialogOpen) {
      loadedKey = "";
      return;
    }
    const target = explorerState.editingSshTarget;
    const key = target?.id ?? "new";
    if (loadedKey === key) return;
    loadedKey = key;
    const configuration = target?.configuration;
    name = configuration?.name ?? "";
    host = configuration?.host ?? "";
    port = configuration?.port ?? 22;
    username = configuration?.username ?? "";
    initialPath = configuration?.initialPath ?? "";
    identityFile = configuration?.identityFile ?? "";
    identitiesOnly = configuration?.identitiesOnly ?? false;
  });

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    await explorerState.saveSshTarget({
      name,
      host,
      port: Number(port),
      username,
      initialPath: initialPath.trim() || null,
      identityFile: identityFile.trim() || null,
      identitiesOnly,
    });
  };
</script>

<Dialog.Root bind:open={explorerState.sshTargetDialogOpen}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title
        >{explorerState.editingSshTarget
          ? "Edit SSH target"
          : "Add SSH target"}</Dialog.Title
      >
      <Dialog.Description>
        Connection details are saved locally. Passwords and passphrases are
        requested only when connecting and are never stored.
      </Dialog.Description>
    </Dialog.Header>

    <form class="grid gap-4" onsubmit={submit}>
      <div class="grid gap-2">
        <Label for="ssh-name">Name</Label>
        <Input
          id="ssh-name"
          bind:value={name}
          placeholder="Production server"
          autocomplete="off"
          maxlength={80}
          required
        />
      </div>

      <div class="grid grid-cols-[1fr_7rem] gap-3">
        <div class="grid gap-2">
          <Label for="ssh-host">Host</Label>
          <Input
            id="ssh-host"
            bind:value={host}
            placeholder="server.example.com"
            autocomplete="off"
            maxlength={255}
            required
          />
        </div>
        <div class="grid gap-2">
          <Label for="ssh-port">Port</Label>
          <Input
            id="ssh-port"
            type="number"
            bind:value={port}
            min={1}
            max={65535}
            required
          />
        </div>
      </div>

      <div class="grid gap-2">
        <Label for="ssh-username">Username</Label>
        <Input
          id="ssh-username"
          bind:value={username}
          placeholder="deploy"
          autocomplete="username"
          maxlength={128}
          required
        />
      </div>

      <div class="grid gap-2">
        <Label for="ssh-path">Starting folder</Label>
        <Input
          id="ssh-path"
          bind:value={initialPath}
          placeholder="~ or /srv/app"
          autocomplete="off"
        />
        <p class="text-xs text-muted-foreground">
          Leave blank to use the server account's home folder.
        </p>
      </div>

      <div class="grid gap-2">
        <Label for="ssh-identity">Identity file</Label>
        <Input
          id="ssh-identity"
          bind:value={identityFile}
          placeholder="~/.ssh/id_ed25519"
          autocomplete="off"
        />
        <p class="text-xs text-muted-foreground">
          Leave blank to try your SSH agent and standard key files.
        </p>
      </div>

      <div
        class="flex items-center justify-between gap-4 rounded-lg border p-3"
      >
        <div class="grid gap-1">
          <Label for="ssh-identities-only">Use only this identity</Label>
          <p class="text-xs text-muted-foreground">
            Skip keys offered by your SSH agent.
          </p>
        </div>
        <Switch id="ssh-identities-only" bind:checked={identitiesOnly} />
      </div>

      {#if explorerState.sshErrorMessage}
        <p class="text-sm text-destructive" role="alert">
          {explorerState.sshErrorMessage}
        </p>
      {/if}

      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          disabled={explorerState.sshTargetSaving}
          onclick={() => explorerState.closeSshTargetDialog()}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={explorerState.sshTargetSaving}>
          {explorerState.sshTargetSaving ? "Saving…" : "Save target"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
