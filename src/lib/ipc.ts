import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Row, Status } from "@/types";

export const openFile = (path: string) => invoke<Status>("open_file", { path });

export const getStatus = () => invoke<Status>("get_status");

export const getRows = (view: "all" | "filtered", start: number, count: number) =>
  invoke<Row[]>("get_rows", { view, start, count });

export const onIndexProgress = (cb: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>("index:progress", (e) => cb(e.payload));
