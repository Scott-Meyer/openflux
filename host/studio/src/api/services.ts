import { createDaemonServiceClient, type DaemonServiceClient } from "./daemonService";
import { createHostUpdater, type HostUpdater } from "./hostUpdate";
import { createNavigationSource, type NavigationSource } from "./navigation";
import { createThirdPartyClient, type ThirdPartyClient } from "./thirdParty";
import { createUpdatesClient, type UpdatesClient } from "./updates";

/** Everything Studio uses besides the configuration client. Tests pass their own. */
export interface StudioServices {
  updates: UpdatesClient;
  thirdParty: ThirdPartyClient;
  daemonService: DaemonServiceClient;
  hostUpdater: HostUpdater;
  navigation: NavigationSource;
}

export function createStudioServices(overrides: Partial<StudioServices> = {}): StudioServices {
  return {
    updates: overrides.updates ?? createUpdatesClient(),
    thirdParty: overrides.thirdParty ?? createThirdPartyClient(),
    daemonService: overrides.daemonService ?? createDaemonServiceClient(),
    hostUpdater: overrides.hostUpdater ?? createHostUpdater(),
    navigation: overrides.navigation ?? createNavigationSource(),
  };
}
