import type React from "react";
import { useState } from "react";
import App from "./App";
import { createStudioServices, type StudioServices } from "./api/services";
import { DaemonStartup } from "./components/DaemonStartup";

/** Makes sure the background service runs, then shows Studio. */
export const StudioRoot: React.FC<{ services?: StudioServices }> = ({ services: injected }) => {
  const [services] = useState(() => injected ?? createStudioServices());
  const [ready, setReady] = useState(false);
  const [platform, setPlatform] = useState<string | undefined>(undefined);
  if (!ready) {
    return (
      <DaemonStartup
        service={services.daemonService}
        onReady={(status) => {
          setPlatform(status?.service.platform);
          setReady(true);
        }}
      />
    );
  }
  return <App platform={platform} />;
};

export default StudioRoot;
