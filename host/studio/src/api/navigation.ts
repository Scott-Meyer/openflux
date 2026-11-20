import { Channel, invoke } from "@tauri-apps/api/core";

/** Requests to open Studio at a page, from its launch arguments or a later launch. */
export interface NavigationSource {
  initialPage(): Promise<string | null>;
  subscribe(onPage: (page: string) => void): () => void;
}

export function createNavigationSource(): NavigationSource {
  return {
    initialPage: async () => {
      try {
        return (await invoke<string | null>("take_initial_page")) ?? null;
      } catch {
        return null;
      }
    },
    subscribe: (onPage) => {
      let active = true;
      try {
        const ch = new Channel<string>();
        ch.onmessage = (p) => {
          if (active) onPage(p);
        };
        void invoke("subscribe_navigation", { channel: ch }).catch(() => {});
      } catch {
        // Outside the desktop shell there is no launcher to listen to.
      }
      return () => {
        active = false;
      };
    },
  };
}
