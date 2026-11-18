import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Button, Modal, errorMessage } from "../../shared/ui/ui";

interface Props {
  title: string;
  onClose(): void;
  onDelete(): Promise<void>;
  onDeleted(): void;
}

/** Confirmación dentro del webview: no depende de los diálogos de JavaScript. */
export function DeleteMeetingDialog({ title, onClose, onDelete, onDeleted }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const content = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previous = document.activeElement;
    cancel.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);

  const close = () => {
    if (!pending.current) onClose();
  };
  const remove = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    content.current?.focus();
    try {
      await onDelete();
    } catch (e) {
      pending.current = false;
      setBusy(false);
      setError(errorMessage(e));
      return;
    }
    onClose();
    onDeleted();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Tab") return;
    e.preventDefault();
    if (pending.current) return;
    const next = document.activeElement === cancel.current ? confirm.current : cancel.current;
    next?.focus();
  };

  return (
    <Modal label="Eliminar reunión" onClose={close} className="delete-meeting-dialog">
      <div ref={content} tabIndex={-1} onKeyDown={onKeyDown} aria-busy={busy}>
        <h2>¿Eliminar esta reunión?</h2>
        <p>Se eliminarán «{title}» y todo su contenido: audio, transcripción, acta y capturas. No se puede deshacer.</p>
        {error ? <p role="alert" className="error">{error}</p> : null}
        <div className="delete-meeting-actions">
          <Button ref={cancel} capsule disabled={busy} onClick={close}>Cancelar</Button>
          <Button ref={confirm} capsule variant="danger" disabled={busy} onClick={() => void remove()}>
            {busy ? "Eliminando…" : "Eliminar reunión"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
