<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import type { ExplorerState } from "../../app/explorer-state.svelte";

  let { state: explorerState }: { state: ExplorerState } = $props();
  let answers = $state<string[]>([]);
  let loadedPromptId = "";

  $effect(() => {
    const event = explorerState.pendingSshPrompt?.event;
    if (!event || event.promptId === loadedPromptId) return;
    loadedPromptId = event.promptId;
    answers =
      event.event === "authenticationPrompt" ? event.fields.map(() => "") : [];
  });

  const submitAnswers = async (event: SubmitEvent) => {
    event.preventDefault();
    const submitted = [...answers];
    answers = answers.map(() => "");
    await explorerState.answerSshPrompt({
      response: "answers",
      answers: submitted,
    });
    submitted.fill("");
  };
</script>

<Dialog.Root
  open={explorerState.pendingSshPrompt !== null}
  onOpenChange={(open) => {
    if (!open && explorerState.pendingSshPrompt)
      explorerState.cancelSshConnection();
  }}
>
  <Dialog.Content showCloseButton={false}>
    {@const prompt = explorerState.pendingSshPrompt?.event}
    {#if prompt?.event === "hostKeyPrompt"}
      <Dialog.Header>
        <Dialog.Title>Trust this SSH host?</Dialog.Title>
        <Dialog.Description>
          This is the first time Explora has seen {prompt.host}:{prompt.port}.
          Verify the fingerprint before continuing.
        </Dialog.Description>
      </Dialog.Header>
      <div
        class="grid gap-3 rounded-lg border bg-muted/40 p-3 font-mono text-xs"
      >
        <div>
          <p class="font-sans text-muted-foreground">Algorithm</p>
          <p class="mt-1 break-all">{prompt.algorithm}</p>
        </div>
        <div>
          <p class="font-sans text-muted-foreground">SHA256 fingerprint</p>
          <p class="mt-1 break-all">{prompt.fingerprint}</p>
        </div>
      </div>
      <p class="text-xs text-muted-foreground">
        Accepting adds this key to your standard SSH known_hosts file. A changed
        key is always blocked.
      </p>
      <Dialog.Footer>
        <Button
          variant="outline"
          onclick={() => explorerState.cancelSshConnection()}
        >
          Cancel
        </Button>
        <Button
          onclick={() =>
            void explorerState.answerSshPrompt({ response: "accept" })}
        >
          Trust and connect
        </Button>
      </Dialog.Footer>
    {:else if prompt?.event === "authenticationPrompt"}
      <Dialog.Header>
        <Dialog.Title>{prompt.title}</Dialog.Title>
        <Dialog.Description>{prompt.instructions}</Dialog.Description>
      </Dialog.Header>
      <form class="grid gap-4" onsubmit={submitAnswers}>
        {#each prompt.fields as field, index (`${prompt.promptId}:${index}`)}
          <div class="grid gap-2">
            <Label for={`ssh-answer-${index}`}>{field.label}</Label>
            <Input
              id={`ssh-answer-${index}`}
              type={field.secret ? "password" : "text"}
              bind:value={answers[index]}
              autocomplete="off"
              required
            />
          </div>
        {/each}
        <Dialog.Footer>
          <Button
            type="button"
            variant="outline"
            onclick={() => explorerState.cancelSshConnection()}
          >
            Cancel
          </Button>
          <Button type="submit">Continue</Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
