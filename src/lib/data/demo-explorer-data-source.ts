import type {
  ContentKind,
  DirectoryRef,
  FileEntrySummary,
  LocationSummary,
  ManualSshTargetInput,
  PreviewSummary,
  SshTargetSummary,
} from "$lib/contracts/explorer";
import type {
  ConnectSshOptions,
  ExplorerDataSource,
  ListDirectoryOptions,
  WatchVolumesOptions,
} from "$lib/data/explorer-data-source";

const roots: Readonly<Record<string, DirectoryRef>> = {
  home: {
    id: "home",
    locationId: "home",
    name: "Home",
    displayPath: "Home",
  },
  desktop: {
    id: "desktop",
    locationId: "desktop",
    name: "Desktop",
    displayPath: "Home/Desktop",
  },
  documents: {
    id: "documents",
    locationId: "documents",
    name: "Documents",
    displayPath: "Home/Documents",
  },
  downloads: {
    id: "downloads",
    locationId: "downloads",
    name: "Downloads",
    displayPath: "Home/Downloads",
  },
  pictures: {
    id: "pictures",
    locationId: "pictures",
    name: "Pictures",
    displayPath: "Home/Pictures",
  },
  music: {
    id: "music",
    locationId: "music",
    name: "Music",
    displayPath: "Home/Music",
  },
  videos: {
    id: "videos",
    locationId: "videos",
    name: "Movies",
    displayPath: "Home/Movies",
  },
  workspace: {
    id: "workspace",
    locationId: "workspace",
    name: "Workspace",
    displayPath: "Workspace",
  },
  "staging-box": {
    id: "staging-box",
    locationId: "staging-box",
    name: "staging-box",
    displayPath: "staging-box:~/projects",
  },
  "render-node": {
    id: "render-node",
    locationId: "render-node",
    name: "render-node",
    displayPath: "render-node:~",
  },
};

const locations: readonly LocationSummary[] = [
  {
    id: "home",
    name: "Home",
    kind: "local",
    role: "home",
    status: "available",
    displayPath: "Home",
    detail: "Local",
    root: roots.home,
  },
  {
    id: "desktop",
    name: "Desktop",
    kind: "local",
    role: "desktop",
    status: "available",
    displayPath: "Home/Desktop",
    detail: "Local",
    root: roots.desktop,
  },
  {
    id: "documents",
    name: "Documents",
    kind: "local",
    role: "documents",
    status: "available",
    displayPath: "Home/Documents",
    detail: "Local",
    root: roots.documents,
  },
  {
    id: "downloads",
    name: "Downloads",
    kind: "local",
    role: "downloads",
    status: "available",
    displayPath: "Home/Downloads",
    detail: "Local",
    root: roots.downloads,
  },
  {
    id: "pictures",
    name: "Pictures",
    kind: "local",
    role: "pictures",
    status: "available",
    displayPath: "Home/Pictures",
    detail: "Local",
    root: roots.pictures,
  },
  {
    id: "music",
    name: "Music",
    kind: "local",
    role: "music",
    status: "available",
    displayPath: "Home/Music",
    detail: "Local",
    root: roots.music,
  },
  {
    id: "videos",
    name: "Movies",
    kind: "local",
    role: "videos",
    status: "available",
    displayPath: "Home/Movies",
    detail: "Local",
    root: roots.videos,
  },
  {
    id: "workspace",
    name: "Workspace",
    kind: "volume",
    role: "volume",
    status: "available",
    displayPath: "Workspace",
    detail: "1.2 TB available",
    root: roots.workspace,
  },
  {
    id: "staging-box",
    name: "staging-box",
    kind: "ssh",
    role: "ssh",
    status: "connected",
    displayPath: "staging-box:~/projects",
    detail: "SSH · Connected",
    root: roots["staging-box"],
  },
  {
    id: "render-node",
    name: "render-node",
    kind: "ssh",
    role: "ssh",
    status: "offline",
    displayPath: "render-node:~",
    detail: "SSH · Offline",
    root: roots["render-node"],
  },
];

const makeEntry = (
  locationId: string,
  name: string,
  kind: FileEntrySummary["kind"],
  contentKind: ContentKind,
  size: number | null,
  modifiedAt: string,
  detail?: string,
): FileEntrySummary => ({
  reference: { id: `${locationId}:${name}`, locationId },
  name,
  kind,
  contentKind,
  size: size?.toString() ?? null,
  modifiedAt: Date.parse(modifiedAt),
  displayPath: `${roots[locationId].displayPath}/${name}`,
  directory:
    kind === "directory"
      ? {
          id: `${locationId}:${name}`,
          locationId,
          name,
          displayPath: `${roots[locationId].displayPath}/${name}`,
        }
      : null,
  detail,
});

const entriesByLocation: Readonly<Record<string, readonly FileEntrySummary[]>> =
  {
    home: [
      makeEntry(
        "home",
        "Projects",
        "directory",
        "folder",
        null,
        "2026-07-18T18:42:00Z",
        "12 items",
      ),
      makeEntry(
        "home",
        "Photos",
        "directory",
        "folder",
        null,
        "2026-07-17T21:08:00Z",
        "328 items",
      ),
      makeEntry(
        "home",
        "Documents",
        "directory",
        "folder",
        null,
        "2026-07-16T16:14:00Z",
        "47 items",
      ),
      makeEntry(
        "home",
        "Downloads",
        "directory",
        "folder",
        null,
        "2026-07-18T17:54:00Z",
        "19 items",
      ),
      makeEntry(
        "home",
        "explora-notes.md",
        "file",
        "document",
        18_432,
        "2026-07-18T19:24:00Z",
      ),
      makeEntry(
        "home",
        "summer-light.jpg",
        "file",
        "image",
        4_284_811,
        "2026-07-15T03:36:00Z",
        "6240 × 4160",
      ),
      makeEntry(
        "home",
        "handoff.pdf",
        "file",
        "document",
        884_210,
        "2026-07-14T22:11:00Z",
        "8 pages",
      ),
      makeEntry(
        "home",
        "ambient-study.m4a",
        "file",
        "audio",
        8_916_320,
        "2026-07-12T08:44:00Z",
        "03:42",
      ),
      makeEntry(
        "home",
        "app-shell.svelte",
        "file",
        "code",
        12_104,
        "2026-07-18T18:06:00Z",
        "Svelte",
      ),
      makeEntry(
        "home",
        "walkthrough.mov",
        "file",
        "video",
        72_410_332,
        "2026-07-11T11:32:00Z",
        "00:48",
      ),
    ],
    desktop: [
      makeEntry(
        "desktop",
        "Screenshots",
        "directory",
        "folder",
        null,
        "2026-07-18T19:03:00Z",
        "24 items",
      ),
      makeEntry(
        "desktop",
        "release-checklist.md",
        "file",
        "document",
        6_904,
        "2026-07-18T18:18:00Z",
      ),
      makeEntry(
        "desktop",
        "layout-reference.png",
        "file",
        "image",
        1_244_019,
        "2026-07-18T16:25:00Z",
        "1440 × 900",
      ),
    ],
    documents: [
      makeEntry(
        "documents",
        "Design",
        "directory",
        "folder",
        null,
        "2026-07-16T10:12:00Z",
        "8 items",
      ),
      makeEntry(
        "documents",
        "Receipts",
        "directory",
        "folder",
        null,
        "2026-07-01T07:41:00Z",
        "31 items",
      ),
      makeEntry(
        "documents",
        "Explora brief.pdf",
        "file",
        "document",
        642_850,
        "2026-07-18T14:20:00Z",
        "6 pages",
      ),
      makeEntry(
        "documents",
        "ssh-hosts.txt",
        "file",
        "document",
        1_204,
        "2026-07-10T09:52:00Z",
      ),
    ],
    downloads: [],
    pictures: [],
    music: [],
    videos: [],
    workspace: [
      makeEntry(
        "workspace",
        "Archive",
        "directory",
        "folder",
        null,
        "2026-06-28T12:08:00Z",
        "96 items",
      ),
      makeEntry(
        "workspace",
        "Footage",
        "directory",
        "folder",
        null,
        "2026-07-17T23:22:00Z",
        "42 items",
      ),
      makeEntry(
        "workspace",
        "explora-backup.tar.zst",
        "file",
        "archive",
        412_884_992,
        "2026-07-18T02:00:00Z",
      ),
    ],
    "staging-box": [
      makeEntry(
        "staging-box",
        "explora",
        "directory",
        "folder",
        null,
        "2026-07-18T18:38:00Z",
        "SSH · 22 items",
      ),
      makeEntry(
        "staging-box",
        "deploy",
        "directory",
        "folder",
        null,
        "2026-07-18T17:12:00Z",
        "SSH · 7 items",
      ),
      makeEntry(
        "staging-box",
        "README.md",
        "file",
        "document",
        4_280,
        "2026-07-18T18:41:00Z",
        "SSH",
      ),
      makeEntry(
        "staging-box",
        "service.log",
        "file",
        "document",
        244_119,
        "2026-07-18T19:30:00Z",
        "SSH",
      ),
      makeEntry(
        "staging-box",
        "healthcheck.ts",
        "file",
        "code",
        3_822,
        "2026-07-18T16:52:00Z",
        "SSH · TypeScript",
      ),
    ],
    "render-node": [],
  };

const excerpts: Partial<Record<ContentKind, string>> = {
  document:
    "Explora keeps local and remote files in one calm, consistent workspace. This preview is representative demo content.",
  code: 'export const explorer = createExplorer({\n  local: true,\n  remotes: "first-class",\n});',
  image:
    "Image preview rendering will be supplied by the bounded Rust preview pipeline.",
  audio:
    "Audio · Preview playback will use the packaged platform media capabilities.",
  video:
    "Video · Preview playback will use the packaged platform media capabilities.",
  archive:
    "Archive contents are not expanded in the initial preview experience.",
};

const abortError = () => {
  const error = new Error("The demo request was cancelled.");
  error.name = "AbortError";
  return error;
};

const wait = (duration: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    if (signal.aborted) {
      reject(abortError());
      return;
    }

    const timeout = window.setTimeout(resolve, duration);
    signal.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timeout);
        reject(abortError());
      },
      { once: true },
    );
  });

export class DemoExplorerDataSource implements ExplorerDataSource {
  private sshTargets: SshTargetSummary[] = [
    {
      id: "demo:staging-box",
      name: "staging-box",
      source: "manual",
      endpoint: "deploy@staging.example.com",
      status: "connected",
      editable: true,
      connectedLocationId: "staging-box",
      configuration: {
        name: "staging-box",
        host: "staging.example.com",
        port: 22,
        username: "deploy",
        initialPath: "~/projects",
        identityFile: null,
        identitiesOnly: false,
      },
    },
    {
      id: "demo:render-node",
      name: "render-node",
      source: "openSshConfig",
      endpoint: "yunho@render.example.com",
      status: "disconnected",
      editable: false,
      connectedLocationId: null,
      configuration: null,
    },
  ];
  private dynamicLocations = new Map<string, LocationSummary>();
  private dynamicRoots = new Map<string, DirectoryRef>();

  async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    await wait(40, signal);
    return locations;
  }

  async watchVolumes({
    signal,
    onSnapshot,
  }: WatchVolumesOptions): Promise<void> {
    if (signal.aborted) throw abortError();
    onSnapshot({
      revision: 1,
      volumes: locations.filter(({ kind }) => kind === "volume"),
      warning: null,
    });
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  async listSshTargets(
    signal: AbortSignal,
  ): Promise<readonly SshTargetSummary[]> {
    await wait(30, signal);
    return this.sshTargets.map((target) => ({ ...target }));
  }

  async createSshTarget(
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    await wait(40, signal);
    const target: SshTargetSummary = {
      id: `demo:manual:${Date.now()}`,
      name: input.name,
      source: "manual",
      endpoint: `${input.username}@${input.host}${input.port === 22 ? "" : `:${input.port}`}`,
      status: "disconnected",
      editable: true,
      connectedLocationId: null,
      configuration: { ...input },
    };
    this.sshTargets = [...this.sshTargets, target];
    return { ...target };
  }

  async updateSshTarget(
    targetId: string,
    input: ManualSshTargetInput,
    signal: AbortSignal,
  ): Promise<SshTargetSummary> {
    await wait(40, signal);
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target || !target.editable)
      throw new Error("Unknown demo SSH target.");
    const updated: SshTargetSummary = {
      ...target,
      name: input.name,
      endpoint: `${input.username}@${input.host}${input.port === 22 ? "" : `:${input.port}`}`,
      status: "disconnected",
      connectedLocationId: null,
      configuration: { ...input },
    };
    this.sshTargets = this.sshTargets.map((candidate) =>
      candidate.id === targetId ? updated : candidate,
    );
    return { ...updated };
  }

  async deleteSshTarget(targetId: string, signal: AbortSignal): Promise<void> {
    await wait(30, signal);
    this.sshTargets = this.sshTargets.filter(({ id }) => id !== targetId);
  }

  async connectSshTarget(
    targetId: string,
    { signal, onEvent }: ConnectSshOptions,
  ): Promise<LocationSummary> {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) throw new Error("Unknown demo SSH target.");
    onEvent({ event: "state", state: "connecting" }, async () => {});
    await wait(120, signal);
    onEvent({ event: "state", state: "authenticating" }, async () => {});
    await wait(100, signal);

    let location = locations.find(({ name }) => name === target.name);
    if (!location) {
      const locationId = `ssh:${target.id}`;
      const root: DirectoryRef = {
        id: locationId,
        locationId,
        name: target.name,
        displayPath: `${target.name}:~`,
      };
      location = {
        id: locationId,
        name: target.name,
        kind: "ssh",
        role: "ssh",
        status: "connected",
        displayPath: root.displayPath,
        detail: target.endpoint,
        root,
      };
      this.dynamicRoots.set(locationId, root);
      this.dynamicLocations.set(locationId, location);
    } else {
      location = { ...location, status: "connected", detail: target.endpoint };
    }
    this.sshTargets = this.sshTargets.map((candidate) =>
      candidate.id === targetId
        ? {
            ...candidate,
            status: "connected",
            connectedLocationId: location!.id,
          }
        : candidate,
    );
    onEvent({ event: "state", state: "connected" }, async () => {});
    return location;
  }

  async disconnectSshTarget(
    targetId: string,
    signal: AbortSignal,
  ): Promise<void> {
    await wait(30, signal);
    this.sshTargets = this.sshTargets.map((target) =>
      target.id === targetId
        ? { ...target, status: "disconnected", connectedLocationId: null }
        : target,
    );
  }

  async listDirectory(
    directory: DirectoryRef,
    { signal, onStart, onBatch, onComplete }: ListDirectoryOptions,
  ): Promise<void> {
    const root =
      roots[directory.locationId] ??
      this.dynamicRoots.get(directory.locationId);
    const rootEntries = entriesByLocation[directory.id];
    const isKnownChild = Object.values(entriesByLocation)
      .flat()
      .some((entry) => entry.directory?.id === directory.id);
    const entries =
      rootEntries ??
      (this.dynamicRoots.has(directory.locationId) || isKnownChild
        ? []
        : undefined);

    if (!entries || !root) {
      throw new Error(`Unknown demo directory: ${directory.id}`);
    }

    await wait(90, signal);
    onStart({
      directory,
      parent: directory.id === root.id ? null : root,
      breadcrumbs:
        directory.id === root.id
          ? [{ label: root.name, directory: root }]
          : [
              { label: root.name, directory: root },
              { label: directory.name, directory },
            ],
    });
    const splitAt = Math.min(4, entries.length);
    if (splitAt > 0) {
      onBatch({ entries: entries.slice(0, splitAt), replace: true });
    }

    if (entries.length > splitAt) {
      await wait(110, signal);
      onBatch({ entries: entries.slice(splitAt), replace: false });
    }
    onComplete({ skippedEntries: 0 });
  }

  async getPreview(
    entry: FileEntrySummary,
    signal: AbortSignal,
  ): Promise<PreviewSummary> {
    await wait(80, signal);

    const location = locations.find(
      ({ id }) => id === entry.reference.locationId,
    );
    return {
      entryId: entry.reference.id,
      kind: entry.contentKind,
      title: entry.name,
      subtitle:
        entry.kind === "directory"
          ? (entry.detail ?? "Folder")
          : (entry.detail ?? "File"),
      excerpt: excerpts[entry.contentKind],
      details: [
        {
          label: "Location",
          value: location?.displayPath ?? entry.reference.locationId,
        },
        {
          label: "Modified",
          value:
            entry.modifiedAt === null
              ? "Unknown"
              : new Date(entry.modifiedAt).toLocaleString(),
        },
        {
          label: "Size",
          value: entry.size === null ? "—" : `${entry.size} bytes`,
        },
      ],
    };
  }
}
