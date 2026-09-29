export type DeletionShortcut = "trash" | "deletePermanently";

const isMacLike = () =>
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

export const isRenameShortcut = (event: KeyboardEvent) =>
  event.key === "F2" || (isMacLike() && event.key === "Enter");

export const isNewFolderShortcut = (event: KeyboardEvent) =>
  event.key.toLowerCase() === "n" &&
  event.shiftKey &&
  !event.altKey &&
  (isMacLike()
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey);

export const deletionShortcut = (
  event: KeyboardEvent,
): DeletionShortcut | null => {
  if (isMacLike()) {
    if (event.key !== "Backspace" || !event.metaKey || event.ctrlKey) {
      return null;
    }
    return event.altKey ? "deletePermanently" : "trash";
  }
  if (
    event.key !== "Delete" ||
    event.metaKey ||
    event.ctrlKey ||
    event.altKey
  ) {
    return null;
  }
  return event.shiftKey ? "deletePermanently" : "trash";
};

export const clipboardShortcut = (
  event: KeyboardEvent,
): "copy" | "cut" | "paste" | null => {
  const primary = isMacLike()
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
  if (!primary || event.altKey || event.shiftKey) return null;
  switch (event.key.toLowerCase()) {
    case "c":
      return "copy";
    case "x":
      return "cut";
    case "v":
      return "paste";
    default:
      return null;
  }
};
