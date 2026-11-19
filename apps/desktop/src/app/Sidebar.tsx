import { AudioLines, FolderOpen, Pause, Play, Search, Settings, Sparkles, Square } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import type { Levels, SessionSnapshot } from "../api";
import { STEP_LABEL } from "../features/meetings/ProcessingStatus";
import { dbToLevel, formatClock, platformLabel } from "../shared/format";
import { ActaMark } from "../shared/brand/ActaMark";
import { IconButton, Spinner } from "../shared/ui/ui";
import { useApi, useApiEvent } from "./ApiContext";

export type View = "meetings" | "groups" | "ask" | "settings" | "live";

const ICON = { size: 16, strokeWidth: 1.9, "aria-hidden": true } as const;

const ITEMS: { key: View; label: string; icon: ReactNode }[] = [
  { key: "meetings", label: "Reuniones", icon: <AudioLines {...ICON} /> },
  { key: "groups", label: "Grupos", icon: <FolderOpen {...ICON} /> },
  { key: "ask", label: "Preguntar", icon: <Sparkles {...ICON} /> },
  { key: "settings", label: "Ajustes", icon: <Settings {...ICON} /> },
];

function MiniMeter({ label, level }: { label: string; level: number }) {
  return (
    <span className="mini-meter" title={label}>
      <span className="mini-meter-label">{label}</span>
      <span className="mini-meter-track">
        <span className="mini-meter-fill" style={{ transform: `scaleX(${level})` }} />
      </span>
    </span>
  );
}

function StartButton({ onStart }: { onStart(): void }) {
  return (
    <button type="button" className="rec-button" onClick={onStart}>
      <span className="rec-button-dot" aria-hidden />
      Grabar ahora
    </button>
  );
}

/** Control de grabación de la barra lateral: refleja la sesión en vivo. */
function RecordControl() {
  const api = useApi();
  const [snap, setSnap] = useState<SessionSnapshot | null>(null);
  const [levels, setLevels] = useState<Levels | null>(null);
  useEffect(() => {
    void api.sessionGetState().then(setSnap);
  }, [api]);
  useApiEvent("session:state", setSnap);
  useApiEvent("session:levels", setLevels);

  const state = snap?.state ?? { state: "idle" as const };

  if (state.state === "recording" || state.state === "paused") {
    const paused = state.state === "paused";
    return (
      <div className={`rec-card glass${paused ? " rec-card-paused" : ""}`}>
        <div className="rec-card-top" role="status">
          <span className={`rec-dot${paused ? " rec-dot-paused" : ""}`} aria-hidden />
          <span className="rec-card-state">{paused ? "En pausa" : "Grabando"}</span>
          <span className="rec-card-time num">{formatClock(snap?.elapsedMs ?? 0)}</span>
        </div>
        <div className="rec-card-meters" aria-hidden>
          <MiniMeter label="Yo" level={paused || !levels ? 0 : dbToLevel(levels.mic.rmsDbfs)} />
          <MiniMeter label="Otros" level={paused || !levels ? 0 : dbToLevel(levels.system.rmsDbfs)} />
        </div>
        <div className="rec-card-actions">
          <IconButton label={paused ? "Reanudar grabación" : "Pausar grabación"} variant="glass" onClick={() => void (paused ? api.sessionResume() : api.sessionPause()).then(setSnap)}>
            {paused ? <Play size={13} fill="currentColor" aria-hidden /> : <Pause size={13} fill="currentColor" aria-hidden />}
          </IconButton>
          <button type="button" className="btn btn-capsule rec-stop" onClick={() => void api.sessionStop().then(setSnap)} aria-label="Parar grabación">
            <Square size={11} fill="currentColor" aria-hidden />
            Parar
          </button>
        </div>
      </div>
    );
  }

  if (state.state === "detected") {
    const name = state.app.platform === "other" ? state.app.name : platformLabel(state.app.platform);
    return (
      <div className="rec-card glass">
        <p className="rec-card-hint">Reunión de {name} en curso</p>
        <button type="button" className="rec-button" onClick={() => void api.promptAnswer(state.prompt.id, "accept").then(setSnap)}>
          <span className="rec-button-dot" aria-hidden />
          Grabar reunión
        </button>
      </div>
    );
  }

  // Procesar una reunión no bloquea grabar otra (igual que en la bandeja):
  // el proceso sigue en segundo plano y el botón se mantiene debajo.
  if (state.state === "processing") {
    return (
      <>
        <div className="rec-card glass rec-card-busy" role="status">
          <Spinner />
          <span>{STEP_LABEL[state.step]}…</span>
        </div>
        <StartButton onStart={() => void api.sessionStart(null).then(setSnap)} />
      </>
    );
  }

  if (state.state === "stopping" || state.state === "recovering") {
    const label = state.state === "stopping" ? "Guardando el audio" : "Recuperando grabación";
    return (
      <div className="rec-card glass rec-card-busy" role="status">
        <Spinner />
        <span>{label}…</span>
      </div>
    );
  }

  return <StartButton onStart={() => void api.sessionStart(null).then(setSnap)} />;
}

interface SidebarProps {
  view: View;
  onView(v: View): void;
  onSearch(): void;
  /** Hay una grabación en curso: sale «En directo». */
  live?: { paused: boolean } | null;
}

export function Sidebar({ view, onView, onSearch, live }: SidebarProps) {
  const api = useApi();
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    api.appInfo().then((i) => setVersion(i.version), () => setVersion(null));
  }, [api]);
  return (
    <aside className="sidebar">
      <div className="side-brand" aria-hidden>
        <ActaMark size={22} />
        <span>acta</span>
      </div>
      <button type="button" className="search-trigger" aria-label="Búsqueda rápida" onClick={onSearch}>
        <Search size={14} strokeWidth={2.1} aria-hidden />
        <span>Buscar</span>
        <span className="kbd" aria-hidden>
          ⌘K
        </span>
      </button>
      <nav aria-label="Secciones" className="side-nav">
        {live ? (
          <button type="button" className={`nav-item nav-live${view === "live" ? " nav-active" : ""}`} aria-current={view === "live" ? "page" : undefined} onClick={() => onView("live")}>
            <span className="nav-live-dot" aria-hidden>
              <span className={`rec-dot rec-dot-sm${live.paused ? " rec-dot-paused" : ""}`} />
            </span>
            En directo
          </button>
        ) : null}
        {ITEMS.map((i) => (
          <button key={i.key} type="button" className={`nav-item${view === i.key ? " nav-active" : ""}${i.key === "settings" ? " nav-bottom" : ""}`} aria-current={view === i.key ? "page" : undefined} onClick={() => onView(i.key)}>
            {i.icon}
            {i.label}
          </button>
        ))}
      </nav>
      <RecordControl />
      {version ? <p className="side-version num muted selectable">v{version}</p> : null}
    </aside>
  );
}
