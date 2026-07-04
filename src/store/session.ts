import { create } from "zustand";
import type { Status } from "@/types";

interface SessionState {
  status: Status;
  setStatus: (s: Status) => void;
}

const EMPTY: Status = { totalLines: 0, indexedBytes: 0, totalBytes: 0, indexing: false };

export const useSession = create<SessionState>()((set) => ({
  status: EMPTY,
  setStatus: (status) => set({ status }),
}));
