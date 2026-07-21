import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DirectoryRef } from "$lib/contracts/explorer";

import { TauriExplorerDataSource } from "./tauri-explorer-data-source";

const root: DirectoryRef = {
  id: "root-token",
  locationId: "home",
  name: "Home",
  displayPath: "/Users/test",
};

const locationPayload = {
  id: "home",
  name: "Home",
  kind: "local",
  role: "home",
  status: "available",
  displayPath: "/Users/test",
  detail: "Local",
  root,
};

const entryPayload = {
  reference: { id: "entry-token", locationId: "home" },
  name: "notes.md",
  kind: "file",
  contentKind: "document",
  size: "5",
  modifiedAt: 1_721_324_000_000,
  displayPath: "/Users/test/notes.md",
  directory: null,
  detail: null,
};

const sshTargetPayload = {
  id: "manual:target-1",
  name: "Staging",
  source: "manual",
  endpoint: "deploy@staging.example.com",
  status: "disconnected",
  editable: true,
  connectedLocationId: null,
  configuration: {
    name: "Staging",
    host: "staging.example.com",
    port: 22,
    username: "deploy",
    initialPath: "/srv/app",
    identityFile: null,
    identitiesOnly: false,
  },
};

const sendChannelMessages = (
  channel: unknown,
  messages: readonly unknown[],
) => {
  const toJson =
    typeof channel === "object" && channel !== null
      ? Reflect.get(channel, "toJSON")
      : undefined;
  const serialized =
    typeof toJson === "function"
      ? Reflect.apply(toJson, channel, [])
      : String(channel);
  const match = /^__CHANNEL__:(\d+)$/.exec(String(serialized));
  if (!match) throw new Error("Expected a Tauri channel identifier.");

  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        runCallback: (id: number, payload: unknown) => void;
      };
    }
  ).__TAURI_INTERNALS__;
  const callbackId = Number(match[1]);
  messages.forEach((message, index) => {
    internals.runCallback(callbackId, { index, message });
  });
  internals.runCallback(callbackId, { index: messages.length, end: true });
};

afterEach(() => clearMocks());

describe("TauriExplorerDataSource", () => {
  it("validates locations and streams typed directory events", async () => {
    mockIPC((command, payload) => {
      if (command === "list_locations") return [locationPayload];
      if (command === "list_directory") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected directory command arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          {
            event: "started",
            directory: root,
            parent: null,
            breadcrumbs: [{ label: "Home", directory: root }],
          },
          { event: "entries", entries: [entryPayload], replace: true },
          { event: "complete", skippedEntries: 0 },
        ]);
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    await expect(source.listLocations(controller.signal)).resolves.toEqual([
      locationPayload,
    ]);
    const onStart = vi.fn();
    const onBatch = vi.fn();
    const onComplete = vi.fn();

    await source.listDirectory(root, {
      signal: controller.signal,
      onStart,
      onBatch,
      onComplete,
    });

    expect(onStart).toHaveBeenCalledWith({
      directory: root,
      parent: null,
      breadcrumbs: [{ label: "Home", directory: root }],
    });
    expect(onBatch).toHaveBeenCalledWith({
      entries: [{ ...entryPayload, detail: undefined }],
      replace: true,
    });
    expect(onComplete).toHaveBeenCalledWith({ skippedEntries: 0 });
  });

  it("rejects malformed IPC data before it reaches explorer state", async () => {
    mockIPC((command) => {
      if (command === "list_locations") {
        return [{ ...locationPayload, kind: "untrusted" }];
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("unknown location kind");
  });

  it("rejects unknown semantic location roles", async () => {
    mockIPC((command) => {
      if (command === "list_locations") {
        return [{ ...locationPayload, role: "untrusted" }];
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    await expect(
      source.listLocations(new AbortController().signal),
    ).rejects.toThrow("unknown location role");
  });

  it("forwards AbortSignal cancellation to the active Rust listing", async () => {
    let finishListing = () => {};
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      if (command === "list_directory") {
        return new Promise<void>((resolve) => {
          finishListing = resolve;
        });
      }
      if (command === "cancel_listing") finishListing();
      return null;
    });

    const source = new TauriExplorerDataSource();
    const controller = new AbortController();
    const listing = source.listDirectory(root, {
      signal: controller.signal,
      onStart: () => {},
      onBatch: () => {},
      onComplete: () => {},
    });

    controller.abort();

    await expect(listing).rejects.toMatchObject({ name: "AbortError" });
    expect(commands).toContain("cancel_listing");
  });

  it("validates saved SSH targets and their editable metadata", async () => {
    mockIPC((command) =>
      command === "list_ssh_targets" ? [sshTargetPayload] : null,
    );

    const source = new TauriExplorerDataSource();
    await expect(
      source.listSshTargets(new AbortController().signal),
    ).resolves.toEqual([sshTargetPayload]);
  });

  it("forwards host trust through a single-use SSH prompt response", async () => {
    const commands: string[] = [];
    let promptResponse: unknown;
    mockIPC((command, payload) => {
      commands.push(command);
      if (command === "connect_ssh_target") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("Expected SSH command arguments.");
        }
        sendChannelMessages(payload.onEvent, [
          {
            event: "hostKeyPrompt",
            promptId: "prompt-1",
            host: "staging.example.com",
            port: 22,
            algorithm: "ssh-ed25519",
            fingerprint: "SHA256:test",
          },
        ]);
        return { ...locationPayload, kind: "ssh", role: "ssh" };
      }
      if (command === "respond_ssh_prompt") {
        promptResponse = payload;
      }
      return null;
    });

    const source = new TauriExplorerDataSource();
    const location = await source.connectSshTarget("manual:target-1", {
      signal: new AbortController().signal,
      onEvent: (event, respond) => {
        if (event.event === "hostKeyPrompt") {
          void respond({ response: "accept" });
        }
      },
    });
    await Promise.resolve();

    expect(location.kind).toBe("ssh");
    expect(commands).toContain("respond_ssh_prompt");
    expect(promptResponse).toMatchObject({
      promptId: "prompt-1",
      response: { response: "accept" },
    });
  });
});
