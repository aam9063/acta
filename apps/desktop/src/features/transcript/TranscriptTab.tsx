import { Captions, Check, Pencil } from "lucide-react";
import { useMemo, useState } from "react";
import type { Speaker, Transcript } from "../../api";
import { formatClock, speakerColor } from "../../shared/format";
import { Highlight } from "../../shared/textSearch";
import { FindBar } from "../../shared/ui/FindBar";
import { useFindShortcut, useLineSearch, type RemoteMatch } from "../../shared/useLineSearch";
import { Avatar, Button, EmptyState, IconButton } from "../../shared/ui/ui";

interface Props {
  transcript: Transcript;
  voiceprintNames: string[];
  currentMs: number;
  onSeek(ms: number): void;
  onRename(speakerId: string, name: string, remember: boolean): void;
  /** Búsqueda del backend (`meeting_search_transcript`); sin ella, o si devuelve `null`, se filtra aquí. */
  onSearch?(query: string): Promise<RemoteMatch[] | null>;
}

const nameOf = (s: Speaker): string => s.displayName ?? s.label;

function RenameForm({ speaker, names, onSave, onCancel }: { speaker: Speaker; names: string[]; onSave(name: string, remember: boolean): void; onCancel(): void }) {
  const [value, setValue] = useState(nameOf(speaker));
  const [remember, setRemember] = useState(true);
  const listId = `vp-${speaker.id}`;
  return (
    <form
      className="rename-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (value.trim()) onSave(value.trim(), remember);
      }}
    >
      <input role="combobox" aria-label="Nombre del hablante" aria-expanded="false" list={listId} value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
      <datalist id={listId}>
        {names.map((n) => (
          <option key={n} value={n} />
        ))}
      </datalist>
      <label className="check">
        <input type="checkbox" className="check-box" aria-label="Recordar esta voz" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
        Recordar esta voz
      </label>
      <Button small variant="primary" type="submit" aria-label="Guardar nombre">
        Guardar
      </Button>
      <Button small variant="ghost" onClick={onCancel}>
        Cancelar
      </Button>
    </form>
  );
}

export function TranscriptTab({ transcript, voiceprintNames, currentMs, onSeek, onRename, onSearch }: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const lines = useMemo(() => transcript.utterances.map((u) => ({ id: u.idx, text: u.text, at: u.start })), [transcript.utterances]);
  const search = useLineSearch(lines, { remote: onSearch, onJump: (hit) => onSeek(hit.at) });
  useFindShortcut(search.input);
  const index = new Map(transcript.speakers.map((s, i) => [s.id, i]));
  const byId = new Map(transcript.speakers.map((s) => [s.id, s]));

  if (transcript.utterances.length === 0) {
    return <EmptyState icon={<Captions size={26} strokeWidth={1.7} />} title="Sin transcripción" hint="Aparecerá aquí cuando termine la transcripción." />;
  }

  return (
    <div className="transcript" ref={search.container}>
      <div className="transcript-find">
        <FindBar label="Buscar en la transcripción" placeholder="Buscar en la transcripción…" query={search.query} onQuery={search.setQuery} count={search.count} index={search.index} onNext={search.next} onPrev={search.prev} inputRef={search.input} />
      </div>
      <ul className="speakers" aria-label="Hablantes">
        {transcript.speakers.map((s, i) => {
          const suggested = !s.isMe && !s.confirmed && (s.nameSource === "voiceprint" || s.nameSource === "appUi" || s.nameSource === "transcript") && s.displayName;
          return (
            <li key={s.id} className={`speaker-chip${editing === s.id ? " speaker-chip-editing" : ""}`}>
              <Avatar name={nameOf(s)} color={speakerColor(i)} size="sm" />
              {editing === s.id ? (
                <RenameForm
                  speaker={s}
                  names={voiceprintNames}
                  onCancel={() => setEditing(null)}
                  onSave={(n, r) => {
                    onRename(s.id, n, r);
                    setEditing(null);
                  }}
                />
              ) : (
                <>
                  <span className="speaker-name">{nameOf(s)}</span>
                  {s.isMe ? <span className="speaker-me">tú</span> : null}
                  {suggested ? (
                    <span className="suggestion">
                      ¿Es {s.displayName}?
                      <Button small capsule variant="primary" icon={<Check size={11} strokeWidth={2.8} aria-hidden />} aria-label={`Confirmar a ${s.displayName}`} onClick={() => onRename(s.id, s.displayName!, true)}>
                        Confirmar
                      </Button>
                    </span>
                  ) : null}
                  {!s.isMe ? (
                    <IconButton small label={`Renombrar a ${nameOf(s)}`} onClick={() => setEditing(s.id)}>
                      <Pencil size={11} strokeWidth={2.2} aria-hidden />
                    </IconButton>
                  ) : null}
                </>
              )}
            </li>
          );
        })}
      </ul>
      <ol className="lines card">
        {transcript.utterances.map((u, i) => {
          const s = byId.get(u.speakerId);
          const active = currentMs >= u.start && currentMs < u.end;
          const first = transcript.utterances[i - 1]?.speakerId !== u.speakerId;
          const color = speakerColor(index.get(u.speakerId) ?? 0);
          const name = s ? nameOf(s) : "?";
          const found = search.rangesFor(u.idx);
          return (
            <li key={u.idx} className={first ? "turn" : undefined}>
              <button type="button" className={`line${active ? " line-active" : ""}`} aria-current={active ? "true" : undefined} onClick={() => onSeek(u.start)}>
                <span className="line-gutter">{first ? <Avatar name={name} color={color} /> : null}</span>
                <span className="line-body">
                  {first ? (
                    <span className="line-speaker" style={{ color }}>
                      {name}
                    </span>
                  ) : null}
                  <span className="line-text">
                    <Highlight text={u.text} ranges={found.ranges} activeIndex={found.activeIndex} />
                  </span>
                </span>
                <span className="line-time num">{formatClock(u.start)}</span>
              </button>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
