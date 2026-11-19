import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { sampleMeeting } from "../../api/mock";
import { TranscriptTab } from "./TranscriptTab";

const d = sampleMeeting();

function setup(over: Partial<React.ComponentProps<typeof TranscriptTab>> = {}) {
  const props = {
    transcript: d.transcript,
    voiceprintNames: ["Ana", "Luis"],
    currentMs: 0,
    onSeek: vi.fn(),
    onRename: vi.fn(),
    ...over,
  };
  render(<TranscriptTab {...props} />);
  return props;
}

describe("TranscriptTab", () => {
  it("muestra cada línea con su hablante", () => {
    setup();
    expect(screen.getByText("Empezamos con el plan.")).toBeInTheDocument();
    expect(screen.getAllByText("Ana").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Edu").length).toBeGreaterThan(0);
  });

  it("al pulsar una línea salta a su inicio", async () => {
    const p = setup();
    await userEvent.click(screen.getByRole("button", { name: /Yo envío el presupuesto/ }));
    expect(p.onSeek).toHaveBeenCalledWith(4_500);
  });

  it("marca la línea que suena", () => {
    setup({ currentMs: 5_000 });
    expect(screen.getByRole("button", { name: /Yo envío el presupuesto/ })).toHaveAttribute("aria-current", "true");
  });

  it("renombra un hablante recordando su voz", async () => {
    const p = setup();
    await userEvent.click(screen.getByRole("button", { name: "Renombrar a Ana" }));
    const input = screen.getByRole("combobox", { name: "Nombre del hablante" });
    await userEvent.clear(input);
    await userEvent.type(input, "Luis");
    await userEvent.click(screen.getByRole("button", { name: "Guardar nombre" }));
    expect(p.onRename).toHaveBeenCalledWith(d.transcript.speakers[1]!.id, "Luis", true);
  });

  it("puede renombrar sin recordar la voz", async () => {
    const p = setup();
    await userEvent.click(screen.getByRole("button", { name: "Renombrar a Ana" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Recordar esta voz" }));
    await userEvent.click(screen.getByRole("button", { name: "Guardar nombre" }));
    expect(p.onRename).toHaveBeenCalledWith(d.transcript.speakers[1]!.id, "Ana", false);
  });

  it("confirma una sugerencia de nombre sin confirmar", async () => {
    const p = setup();
    expect(screen.getByText(/¿Es Ana\?/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Confirmar a Ana" }));
    expect(p.onRename).toHaveBeenCalledWith(d.transcript.speakers[1]!.id, "Ana", true);
  });

  it("sugiere el nombre deducido de la conversación y al confirmarlo recuerda la voz", async () => {
    const base = sampleMeeting().transcript;
    const transcript = { ...base, speakers: base.speakers.map((s, i) => (i === 1 ? { ...s, displayName: "Marta", nameSource: "transcript" as const } : s)) };
    const p = setup({ transcript });
    expect(screen.getByText(/¿Es Marta\?/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Confirmar a Marta" }));
    expect(p.onRename).toHaveBeenCalledWith(base.speakers[1]!.id, "Marta", true);
  });

  it("no ofrece renombrar al propio usuario", () => {
    setup();
    expect(screen.queryByRole("button", { name: "Renombrar a Edu" })).not.toBeInTheDocument();
  });
});

describe("turnos del transcript", () => {
  it("agrupa las frases seguidas del mismo hablante en un turno", () => {
    const base = sampleMeeting().transcript;
    const [ana, me] = [base.speakers[1]!, base.speakers[0]!];
    const line = (idx: number, speakerId: string, text: string) => ({ idx, speakerId, track: "system" as const, start: idx * 1000, end: idx * 1000 + 900, text, confidence: null });
    render(
      <TranscriptTab
        transcript={{ ...base, utterances: [line(0, ana.id, "Uno."), line(1, ana.id, "Dos."), line(2, me.id, "Tres.")] }}
        voiceprintNames={[]}
        currentMs={0}
        onSeek={() => {}}
        onRename={() => {}}
      />,
    );
    const items = screen.getAllByRole("listitem").filter((li) => li.querySelector(".line"));
    expect(items.map((li) => li.className)).toEqual(["turn", "", "turn"]);
    // El nombre solo encabeza el turno (más la ficha de hablantes).
    expect(screen.getAllByText("Ana")).toHaveLength(2);
  });

  it("marca quién eres tú", () => {
    setup();
    expect(screen.getByText("tú")).toBeInTheDocument();
  });
});

describe("buscar en el transcript", () => {
  const long = (() => {
    const t = sampleMeeting().transcript;
    const [ana, me] = [t.speakers[1]!.id, t.speakers[0]!.id];
    t.utterances = [
      { idx: 0, speakerId: ana, track: "system", start: 0, end: 4_000, text: "Empezamos con el plan de la reunión.", confidence: 1 },
      { idx: 1, speakerId: me, track: "mic", start: 5_000, end: 9_000, text: "El plan está claro, el PLAN es lanzar.", confidence: 1 },
      { idx: 2, speakerId: ana, track: "system", start: 10_000, end: 12_000, text: "Hablemos de las subcuentas.", confidence: 1 },
    ];
    return t;
  })();
  const box = () => screen.getByRole("searchbox", { name: "Buscar en la transcripción" });

  it("resalta las coincidencias y las cuenta, sin distinguir tildes ni mayúsculas", async () => {
    setup({ transcript: long });
    await userEvent.type(box(), "plan");
    expect(await screen.findByText("1 de 3")).toBeInTheDocument();
    expect(document.querySelectorAll("mark.hit")).toHaveLength(3);
    await userEvent.clear(box());
    await userEvent.type(box(), "reunion");
    expect(await screen.findByText("1 de 1")).toBeInTheDocument();
    expect(document.querySelector("mark.hit")?.textContent).toBe("reunión");
  });

  it("siguiente y anterior recorren las coincidencias y saltan el audio", async () => {
    const p = setup({ transcript: long });
    await userEvent.type(box(), "plan");
    await screen.findByText("1 de 3");
    await userEvent.click(screen.getByRole("button", { name: "Siguiente coincidencia" }));
    expect(screen.getByText("2 de 3")).toBeInTheDocument();
    expect(p.onSeek).toHaveBeenLastCalledWith(5_000);
    expect(document.querySelectorAll("mark.hit-active")).toHaveLength(1);
    await userEvent.type(box(), "{Enter}");
    expect(screen.getByText("3 de 3")).toBeInTheDocument();
    await userEvent.type(box(), "{Enter}");
    expect(screen.getByText("1 de 3")).toBeInTheDocument();
    expect(p.onSeek).toHaveBeenLastCalledWith(0);
    await userEvent.type(box(), "{Shift>}{Enter}{/Shift}");
    expect(screen.getByText("3 de 3")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Coincidencia anterior" }));
    expect(screen.getByText("2 de 3")).toBeInTheDocument();
  });

  it("sin coincidencias lo dice y Esc limpia", async () => {
    setup({ transcript: long });
    await userEvent.type(box(), "presupuesto");
    expect(await screen.findByText("Sin resultados")).toBeInTheDocument();
    await userEvent.type(box(), "{Escape}");
    expect(box()).toHaveValue("");
    expect(screen.queryByText("Sin resultados")).not.toBeInTheDocument();
  });

  it("⌘F lleva al buscador", async () => {
    setup({ transcript: long });
    await userEvent.keyboard("{Meta>}f{/Meta}");
    expect(box()).toHaveFocus();
  });

  it("usa la búsqueda del backend y resalta la línea entera si no encuentra el texto literal", async () => {
    const onSearch = vi.fn(async (q: string) => (q === "cuentas hijas" ? [{ utteranceId: 2, startMs: 10_000, snippet: "Hablemos de las subcuentas.", ranges: [] as [number, number][] }] : []));
    setup({ transcript: long, onSearch });
    await userEvent.type(box(), "cuentas hijas");
    expect(await screen.findByText("1 de 1")).toBeInTheDocument();
    expect(onSearch).toHaveBeenLastCalledWith("cuentas hijas");
    expect(document.querySelector("mark.hit")?.textContent).toBe("Hablemos de las subcuentas.");
  });

  it("si el backend no sabe buscar (null) o falla, filtra en cliente", async () => {
    const onSearch = vi.fn(async () => null);
    setup({ transcript: long, onSearch });
    await userEvent.type(box(), "subcuentas");
    expect(await screen.findByText("1 de 1")).toBeInTheDocument();
  });
});
