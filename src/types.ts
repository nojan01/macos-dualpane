export type Entry = {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  mtime: number; // unix seconds
  ext: string;
  hidden: boolean;
  // Extended (optional, populated by backend)
  birthTime?: number; // unix seconds
  kind?: string;
  owner?: string;
  group?: string;
  modeStr?: string; // e.g. "rwxr-xr-x"
};

export type PaneId = "left" | "right";

export type SortKey = "name" | "size" | "mtime";
export type SortDir = "asc" | "desc";
