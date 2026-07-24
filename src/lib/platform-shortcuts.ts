export type DeletionShortcut = "trash" | "deletePermanently";

const isMacLike = () =>
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

export const isRenameShortcut = (event: KeyboardEvent) =>
  event.key === "F2" || (isMacLike() && event.key === "Enter");

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
