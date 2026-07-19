import type { FileEntrySummary } from "$lib/contracts/explorer";

export const compareFileSizes = (
  left: FileEntrySummary,
  right: FileEntrySummary,
): number => {
  if (left.size === right.size) return 0;
  if (left.size === null) return -1;
  if (right.size === null) return 1;

  const leftSize = BigInt(left.size);
  const rightSize = BigInt(right.size);
  return leftSize < rightSize ? -1 : 1;
};

export const formatFileSize = (size: string | null): string => {
  if (size === null) return "—";

  const bytes = Number(size);
  if (!Number.isFinite(bytes)) return `${size} bytes`;
  if (bytes < 1_000) return `${bytes} B`;

  const units = ["KB", "MB", "GB", "TB", "PB"];
  let value = bytes / 1_000;
  let unit = units[0];

  for (let index = 1; index < units.length && value >= 1_000; index += 1) {
    value /= 1_000;
    unit = units[index];
  }

  return `${value.toLocaleString(undefined, {
    maximumFractionDigits: value >= 100 ? 0 : 1,
  })} ${unit}`;
};
