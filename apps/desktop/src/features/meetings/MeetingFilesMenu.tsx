import { Download } from "lucide-react";
import { useRef } from "react";
import type { MeetingId } from "../../api";
import { useApi } from "../../app/ApiContext";
import { errorMessage } from "../../shared/ui/ui";
import { usePopover } from "../../shared/ui/usePopover";

interface Props {
  meetingId: MeetingId;
  hasTranscript: boolean;
  hasAudio: boolean;
  /** Resultado de cada acción: `ok` con un aviso o `error` con el motivo. */
  onResult(r: { ok: string } | { error: string }): void;
}

/** «Descargar»: transcripción (.txt / .md), audio y carpeta de la reunión. */
export function MeetingFilesMenu({ meetingId, hasTranscript, hasAudio, onResult }: Props) {
  const api = useApi();
  const { open, setOpen, close, root, trigger } = usePopover<HTMLSpanElement>();
  const menu = useRef<HTMLDivElement>(null);

  const run = (job: () => Promise<string>) => {
    close();
    job().then(
      (ok) => onResult({ ok }),
      (e: unknown) => onResult({ error: errorMessage(e) }),
    );
  };
  const saved = "Guardado en Descargas.";

  return (
    <span className="split" ref={root}>
      <button ref={trigger} type="button" className="btn icon-btn btn-glass btn-capsule" aria-label="Descargar" title="Descargar" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Download size={15} strokeWidth={1.9} aria-hidden />
      </button>
      {open ? (
        <div ref={menu} role="menu" aria-label="Descargar" tabIndex={-1} className="menu glass-thick">
          <button type="button" role="menuitem" className="menu-item" disabled={!hasTranscript} onClick={() => run(() => api.meetingExportTranscript(meetingId, "txt").then(() => saved))}>
            Transcripción (.txt)
          </button>
          <button type="button" role="menuitem" className="menu-item" disabled={!hasTranscript} onClick={() => run(() => api.meetingExportTranscript(meetingId, "md").then(() => saved))}>
            Transcripción (.md)
          </button>
          <button type="button" role="menuitem" className="menu-item" disabled={!hasAudio} onClick={() => run(() => api.meetingExportAudio(meetingId).then(() => saved))}>
            Audio (.wav)
          </button>
          <button type="button" role="menuitem" className="menu-item" onClick={() => run(() => api.openMeetingFolder(meetingId).then(() => ""))}>
            Abrir carpeta de la reunión
          </button>
        </div>
      ) : null}
    </span>
  );
}
