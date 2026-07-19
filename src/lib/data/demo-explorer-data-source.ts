import type {
  ContentKind,
  FileEntrySummary,
  LocationSummary,
  PreviewSummary,
} from "$lib/contracts/explorer";
import type {
  ExplorerDataSource,
  ListDirectoryOptions,
} from "$lib/data/explorer-data-source";

const locations: readonly LocationSummary[] = [
  {
    id: "home",
    name: "Home",
    kind: "local",
    status: "available",
    displayPath: "Home",
    detail: "Local",
  },
  {
    id: "desktop",
    name: "Desktop",
    kind: "local",
    status: "available",
    displayPath: "Home/Desktop",
    detail: "Local",
  },
  {
    id: "documents",
    name: "Documents",
    kind: "local",
    status: "available",
    displayPath: "Home/Documents",
    detail: "Local",
  },
  {
    id: "workspace",
    name: "Workspace",
    kind: "volume",
    status: "available",
    displayPath: "Workspace",
    detail: "1.2 TB available",
  },
  {
    id: "staging-box",
    name: "staging-box",
    kind: "ssh",
    status: "connected",
    displayPath: "staging-box:~/projects",
    detail: "SSH · Connected",
  },
  {
    id: "render-node",
    name: "render-node",
    kind: "ssh",
    status: "offline",
    displayPath: "render-node:~",
    detail: "SSH · Offline",
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
  id: `${locationId}:${name}`,
  locationId,
  name,
  kind,
  contentKind,
  size,
  modifiedAt,
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
  async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    await wait(40, signal);
    return locations;
  }

  async listDirectory(
    locationId: string,
    { signal, onBatch }: ListDirectoryOptions,
  ): Promise<void> {
    const entries = entriesByLocation[locationId];

    if (!entries) {
      throw new Error(`Unknown demo location: ${locationId}`);
    }

    await wait(90, signal);
    const splitAt = Math.min(4, entries.length);
    onBatch({ entries: entries.slice(0, splitAt), replace: true });

    if (entries.length > splitAt) {
      await wait(110, signal);
      onBatch({ entries: entries.slice(splitAt), replace: false });
    }
  }

  async getPreview(
    entry: FileEntrySummary,
    signal: AbortSignal,
  ): Promise<PreviewSummary> {
    await wait(80, signal);

    const location = locations.find(({ id }) => id === entry.locationId);
    return {
      entryId: entry.id,
      kind: entry.contentKind,
      title: entry.name,
      subtitle:
        entry.kind === "directory"
          ? (entry.detail ?? "Folder")
          : (entry.detail ?? "File"),
      excerpt: excerpts[entry.contentKind],
      details: [
        { label: "Location", value: location?.displayPath ?? entry.locationId },
        { label: "Modified", value: entry.modifiedAt },
        {
          label: "Size",
          value: entry.size === null ? "—" : `${entry.size} bytes`,
        },
      ],
    };
  }
}
