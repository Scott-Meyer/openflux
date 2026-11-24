import { useEffect, useId, useRef, useState } from "react";
import { toDaemonError } from "../api/daemonError";
import { type BindingLayout, type V2ModuleRecord, v2Client } from "../api/v2Client";
import { formatKeyActionLabel } from "../types/openflux";

const MAX_IMPORT_BYTES = 512 * 1024;
const MODIFIERS = [
  [1, "Ctrl"],
  [2, "Shift"],
  [4, "Alt"],
  [8, "Cmd"],
] as const;

function shortcutLabel(key: string, mods: number): string {
  const modifiers = MODIFIERS.filter(([mask]) => mods & mask).map(([, name]) => name);
  const label = formatKeyActionLabel({ keyCode: key }).replace(/^KP_/, "Keypad ").replace(/_/g, " ");
  return [...modifiers, label].join(" + ");
}

function readExport(file: File): Promise<string> {
  if (!/\.txt$/i.test(file.name)) throw new Error("Choose a Myslot .txt export.");
  if (file.size > MAX_IMPORT_BYTES) throw new Error("The export is larger than 512 KiB.");
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      try {
        resolve(new TextDecoder("utf-8", { fatal: true }).decode(reader.result as ArrayBuffer));
      } catch {
        reject(new Error("The export is not valid UTF-8 text."));
      }
    };
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the selected file."));
    reader.readAsArrayBuffer(file);
  });
}

export function BindingImportPanel({ moduleId, moduleSnapshot }: { moduleId: string; moduleSnapshot?: V2ModuleRecord }) {
  const titleId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const previousSnapshot = useRef(moduleSnapshot);
  const [file, setFile] = useState<File | null>(null);
  const [layout, setLayout] = useState<BindingLayout | null | undefined>();
  const [activity, setActivity] = useState<"loading" | "importing" | null>("loading");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    // The Studio shell refreshes module records whenever the shared daemon revision changes.
    // Re-read the current layout even when the import was done by an agent in another client.
    if (activity === "importing" || previousSnapshot.current === moduleSnapshot) return;
    previousSnapshot.current = moduleSnapshot;
    setActivity("loading");
  }, [activity, moduleSnapshot]);

  useEffect(() => {
    if (activity !== "loading") return;
    let alive = true;
    void v2Client
      .getBindingLayout(moduleId)
      .then(({ layout }) => {
        if (alive) setLayout(layout);
      })
      .catch((error) => {
        if (alive) setError(`Could not load imported bindings: ${toDaemonError(error).message}`);
      })
      .finally(() => {
        if (alive) setActivity(null);
      });
    return () => {
      alive = false;
    };
  }, [moduleId, activity, moduleSnapshot]);

  function refresh() {
    setActivity("loading");
    setError(null);
    setNotice(null);
  }

  async function importSelected() {
    if (!file || activity !== null) return;
    setActivity("importing");
    setError(null);
    setNotice(null);
    try {
      const contents = await readExport(file);
      const result = await v2Client.importBindingLayout(moduleId, contents);
      setLayout(result.layout);
      if (result.layout) setNotice(`Imported ${file.name} on this computer.`);
      setFile(null);
    } catch (error) {
      setError(`Could not import ${file.name}: ${toDaemonError(error).message}`);
    } finally {
      setActivity(null);
    }
  }

  return (
    <section className="kg-calm-card kg-settings-stack" aria-labelledby={titleId}>
      <div>
        <h2 id={titleId} className="kg-row-title" style={{ margin: 0 }}>
          Import bindings
        </h2>
        <div className="kg-muted">Save your WoW Myslot export as a UTF-8 .txt file (up to 512 KiB).</div>
      </div>
      <div className="kg-strip-top">
        <div className="kg-strip-top">
          <input
            ref={inputRef}
            type="file"
            accept=".txt"
            aria-label="Myslot export file"
            hidden
            disabled={activity !== null}
            onChange={(event) => {
              const selected = event.currentTarget.files?.[0];
              if (selected) setFile(selected);
              // Allow re-selecting a file with the same name after editing the export.
              event.currentTarget.value = "";
            }}
          />
          <button
            type="button"
            className="kg-btn-ghost"
            disabled={activity !== null}
            onClick={() => inputRef.current?.click()}
          >
            Choose .txt file…
          </button>
          <span className="kg-muted" style={{ overflowWrap: "anywhere" }}>
            {file ? `Selected: ${file.name}` : "No file selected"}
          </span>
        </div>
        <button
          type="button"
          className="kg-btn-primary"
          disabled={!file || activity !== null}
          onClick={() => void importSelected()}
        >
          Import
        </button>
      </div>
      <div className="kg-muted">
        Import saves bindings on this computer. Use <strong>Sync to Keyboard</strong> after importing to send them to
        the keyboard.
      </div>
      <div className="kg-strip-top">
        <h3 className="kg-row-title" style={{ margin: 0 }}>
          Imported bindings
        </h3>
        <button type="button" className="kg-btn-ghost" disabled={activity !== null} onClick={refresh}>
          Refresh bindings
        </button>
      </div>
      <div role="status" className="kg-muted" style={{ overflowWrap: "anywhere" }}>
        {activity === "loading" ? "Loading imported bindings…" : activity === "importing" ? "Importing…" : notice}
      </div>
      {error && (
        <div role="alert" className="kg-error-banner" style={{ overflowWrap: "anywhere" }}>
          {error}
        </div>
      )}
      {layout === null && <div className="kg-muted">No bindings have been imported yet.</div>}
      {layout && (
        <>
          <div className="kg-muted">
            {layout.faces.length} {layout.faces.length === 1 ? "binding" : "bindings"}
            {layout.flavor ? ` · ${layout.flavor}` : ""}
            {layout.skipped !== undefined ? ` · ${layout.skipped} skipped` : ""}
          </div>
          {layout.faces.length > 0 && (
            <ul className="kg-actions-list" aria-label="Imported bindings" style={{ margin: 0, padding: 0 }}>
              {layout.faces.map((face) => (
                <li
                  key={`${face.key}:${face.mods}`}
                  className="kg-action-row"
                  style={{ gap: 12, overflowWrap: "anywhere" }}
                >
                  <kbd>{shortcutLabel(face.key, face.mods)}</kbd>
                  <span>{face.name}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}
