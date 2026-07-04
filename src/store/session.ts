import { create } from "zustand";
import type { Status } from "@/types";

interface SessionState {
  status: Status;
  sessionId: number; // 每打开一个新文件自增,供表格清缓存
  setStatus: (s: Status) => void;
  beginSession: (s: Status) => void;
}

const EMPTY: Status = { totalLines: 0, indexedBytes: 0, totalBytes: 0, indexing: false };

export const useSession = create<SessionState>()((set) => ({
  status: EMPTY,
  sessionId: 0,
  // 索引进度事件用它更新状态(不换 session)。
  setStatus: (status) => set({ status }),
  // 打开新文件时用它:更新状态并自增 sessionId。
  beginSession: (status) => set((s) => ({ status, sessionId: s.sessionId + 1 })),
}));
