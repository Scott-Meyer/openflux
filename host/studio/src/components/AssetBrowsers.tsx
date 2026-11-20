import type React from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import { useMountedRef } from "../hooks/useMountedRef";
import type { AssetCatalog } from "../types/openflux";

interface FontBrowserModalProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  currentFamily: string;
  onSelect: (family: string) => void;
  onClose: () => void;
}

const registeredSpecimenFonts = new Set<string>();

export const FontBrowserModal: React.FC<FontBrowserModalProps> = ({
  client,
  catalog,
  currentFamily,
  onSelect,
  onClose,
}) => {
  const [query, setQuery] = useState("");
  const [fontStates, setFontStates] = useState<Record<string, "loading" | "loaded" | "error">>({});
  const inFlightFontsRef = useRef<Set<string>>(new Set());
  const fonts = useMemo(
    () => (catalog?.fonts ?? []).filter((f) => f.family.toLowerCase().includes(query.trim().toLowerCase())),
    [catalog?.fonts, query],
  );
  const mountedRef = useMountedRef();

  useEffect(() => {
    if (typeof FontFace === "undefined" || !document.fonts) return;

    fonts.forEach((f) => {
      const repFile = f.files.find((fn) => !fn.toLowerCase().includes("italic")) ?? f.files[0];
      if (!repFile) return;
      const specFamily = `Specimen_${f.family}`;
      if (registeredSpecimenFonts.has(specFamily)) {
        setFontStates((prev) => (prev[f.family] === "loaded" ? prev : { ...prev, [f.family]: "loaded" }));
        return;
      }
      if (inFlightFontsRef.current.has(f.family)) return;
      inFlightFontsRef.current.add(f.family);
      setFontStates((prev) => (prev[f.family] ? prev : { ...prev, [f.family]: "loading" }));
      void client
        .getAssetDataUrl("fonts", repFile)
        .then(async (url) => {
          const face = new FontFace(specFamily, `url("${url}")`);
          const loaded = await face.load();
          document.fonts.add(loaded);
          registeredSpecimenFonts.add(specFamily);
          if (mountedRef.current) {
            setFontStates((prev) => ({ ...prev, [f.family]: "loaded" }));
          }
        })
        .catch(() => {
          inFlightFontsRef.current.delete(f.family);
          if (mountedRef.current) {
            setFontStates((prev) => ({ ...prev, [f.family]: "error" }));
          }
        });
    });
  }, [fonts, client, mountedRef]);

  return (
    <div className="modal-backdrop" role="dialog" aria-label="Font Browser">
      <div className="modal-card modal-wide">
        <div className="toolbar-between">
          <h3>Font Family Browser ({fonts.length} fonts)</h3>
          <button type="button" className="btn-secondary" onClick={onClose}>
            Close
          </button>
        </div>
        <input
          type="text"
          className="search-input"
          placeholder="Search fonts (Montserrat, Inter, JetBrainsMono, BebasNeue...)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="browser-grid">
          {fonts.map((f) => {
            const st = fontStates[f.family];
            const specCss =
              st === "loaded" ? `"Specimen_${f.family}", "${f.family}", sans-serif` : `"${f.family}", sans-serif`;
            return (
              <button
                key={f.family}
                type="button"
                className={`browser-item ${currentFamily === f.family ? "selected" : ""}`}
                onClick={() => {
                  onSelect(f.family);
                  onClose();
                }}
              >
                <div className="specimen-lg" style={{ fontFamily: specCss }}>
                  Abc 12:45
                </div>
                <strong>{f.family}</strong>
                <small className="muted">
                  {f.isVariable ? "Variable" : `${f.files.length} style(s)`}
                  {f.hasItalic ? " · Italic" : ""}
                  {st === "loading" ? " · Loading" : st === "error" ? " · System fallback" : ""}
                </small>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
};

interface IconBrowserModalProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  currentIcon: string;
  onSelect: (iconId: string) => void;
  onClose: () => void;
}

export const IconBrowserModal: React.FC<IconBrowserModalProps> = ({
  client,
  catalog,
  currentIcon,
  onSelect,
  onClose,
}) => {
  const [query, setQuery] = useState("");
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const inFlightIconsRef = useRef<Set<string>>(new Set());
  const loadedIconsRef = useRef<Set<string>>(new Set());
  const visibleIcons = useMemo(
    () =>
      (catalog?.keyIcons ?? [])
        .filter(
          (ic) =>
            ic.name.toLowerCase().includes(query.trim().toLowerCase()) ||
            ic.id.toLowerCase().includes(query.trim().toLowerCase()),
        )
        .slice(0, 120),
    [catalog?.keyIcons, query],
  );
  const mountedRef = useMountedRef();

  useEffect(() => {
    visibleIcons.forEach((ic) => {
      if (loadedIconsRef.current.has(ic.id) || inFlightIconsRef.current.has(ic.id)) return;
      inFlightIconsRef.current.add(ic.id);
      void client
        .getAssetDataUrl(ic.category, ic.id)
        .then((url) => {
          inFlightIconsRef.current.delete(ic.id);
          loadedIconsRef.current.add(ic.id);
          if (mountedRef.current) {
            setThumbs((prev) => ({ ...prev, [ic.id]: url }));
          }
        })
        .catch(() => {
          inFlightIconsRef.current.delete(ic.id);
        });
    });
  }, [visibleIcons, client, mountedRef]);

  const [status, setStatus] = useState<string | null>(null);

  return (
    <div className="modal-backdrop" role="dialog" aria-label="Icon Browser">
      <div className="modal-card modal-wide">
        <div className="toolbar-between">
          <h3>Key Icon Picker ({visibleIcons.length} shown)</h3>
          <button type="button" className="btn-secondary" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="toolbar-between" style={{ gap: 10, marginBottom: 8 }}>
          <input
            type="text"
            className="search-input"
            style={{ flex: 1, marginBottom: 0 }}
            placeholder="Filter icons..."
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button
            type="button"
            className="btn-secondary"
            onClick={async () => {
              const p = await client.pickFile("Upload Custom Key Icon", ["png", "jpg", "jpeg", "bmp", "svg"]);
              if (!p?.trim()) return;
              try {
                const res = await client.addIcon(p.trim());
                const id = res.iconPaths[0];
                if (id) {
                  onSelect(id);
                  onClose();
                }
              } catch (e) {
                setStatus(String(e));
              }
            }}
          >
            + Upload Custom Icon...
          </button>
        </div>
        {status && <p className="status-msg">{status}</p>}
        <div className="icon-grid">
          <button
            type="button"
            className={`icon-tile ${currentIcon === "" ? "selected" : ""}`}
            onClick={() => {
              onSelect("");
              onClose();
            }}
          >
            <span className="muted">None</span>
          </button>
          {visibleIcons.map((ic) => (
            <button
              key={ic.id}
              type="button"
              className={`icon-tile ${currentIcon === ic.id ? "selected" : ""}`}
              onClick={() => {
                onSelect(ic.id);
                onClose();
              }}
              title={ic.id}
            >
              {thumbs[ic.id] ? (
                <img src={thumbs[ic.id]} alt={ic.name} className="icon-preview-img" />
              ) : (
                <span className="muted" style={{ fontSize: 10 }}>
                  {ic.category}
                </span>
              )}
              <small>{ic.name.slice(0, 14)}</small>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
};

interface PresetBrowserModalProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  onApplyPreset: (preset: AssetCatalog["presets"][number]) => void;
  onClose: () => void;
}

export const PresetBrowserModal: React.FC<PresetBrowserModalProps> = ({ client, catalog, onApplyPreset, onClose }) => {
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const presets = useMemo(() => catalog?.presets ?? [], [catalog?.presets]);

  useEffect(() => {
    let cancelled = false;
    presets.forEach((p) => {
      const thumbnail = p.thumbnail;
      if (!thumbnail) return;
      // A preset's thumbnail sits in a thumbnails folder; one that names a background image
      // directly is found among the media.
      void client
        .getAssetDataUrl("thumbnails", thumbnail)
        .catch(() => client.getAssetDataUrl("media_themes", thumbnail))
        .then((url) => {
          if (!cancelled) setThumbs((prev) => ({ ...prev, [p.id]: url }));
        })
        .catch(() => {});
    });
    return () => {
      cancelled = true;
    };
  }, [presets, client]);

  return (
    <div className="modal-backdrop" role="dialog" aria-label="Theme Preset Browser">
      <div className="modal-card modal-wide">
        <div className="toolbar-between">
          <h3>Theme Presets Gallery</h3>
          <button type="button" className="btn-secondary" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="browser-grid">
          {presets.map((p) => (
            <button
              key={p.id}
              type="button"
              className="browser-item preset-card"
              onClick={() => {
                onApplyPreset(p);
                onClose();
              }}
            >
              {thumbs[p.id] ? (
                <img src={thumbs[p.id]} alt={p.name} className="preset-thumb" />
              ) : (
                <div className="preset-thumb preset-thumb-empty">{p.fontFamily}</div>
              )}
              <strong>{p.name}</strong>
              <small className="muted">
                {p.fontFamily} · {p.selectedKeyEffect}
              </small>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
};
