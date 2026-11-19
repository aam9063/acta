import type { AppInfo } from "../../api";
import { ActaMark } from "../../shared/brand/ActaMark";
import { Button, Disclosure } from "../../shared/ui/ui";
import { Field, Group, Row, Section } from "./fields";

export function AboutSection({ info, onOpenDataFolder }: { info: AppInfo | null; onOpenDataFolder?: () => void }) {
  return (
    <Section title="Acerca de">
      <div className="about-hero card">
        <ActaMark size={64} />
        <div>
          <p className="about-name">acta</p>
          <p className="muted">Tus reuniones, transcritas y resumidas en tu Mac.</p>
        </div>
      </div>
      <Group>
        <Row label="Versión">
          <span className="muted num selectable">{info?.version ?? "No disponible"}</span>
        </Row>
        <Row label="Carpeta de reuniones" hint="Audio, transcripciones y capturas de todas las reuniones.">
          <Button small onClick={onOpenDataFolder} disabled={!onOpenDataFolder}>
            Abrir carpeta de datos
          </Button>
        </Row>
        <Row label="macOS">
          <span className="muted num selectable">{info?.macosVersion ?? "Desconocido"}</span>
        </Row>
        <Row label="Componente de captura" hint="El ayudante nativo que graba el audio y detecta las reuniones.">
          <span className="muted num selectable">{info?.sidecarVersion ?? "No disponible"}</span>
        </Row>
      </Group>
      <Disclosure label="Avanzado">
        <div className="card advanced">
          <Field label="Carpeta de datos" hint="Aquí están el audio, las capturas, los modelos y la base de datos.">
            <code className="path selectable">{info?.dataDir ?? "No disponible"}</code>
          </Field>
        </div>
      </Disclosure>
    </Section>
  );
}
