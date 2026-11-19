import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockApi } from "../api";
import { App } from "./App";

function setup() {
  const mock = createMockApi();
  render(<App api={mock.api} />);
  return mock;
}

describe("App", () => {
  it("abre en la lista de reuniones y entra al detalle", async () => {
    setup();
    await userEvent.click(await screen.findByRole("button", { name: /Daily del equipo/ }));
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Volver" }));
    expect(await screen.findByRole("searchbox", { name: "Buscar reuniones" })).toBeInTheDocument();
  });

  it("navega a grupos, preguntar y ajustes", async () => {
    setup();
    const nav = screen.getByRole("navigation", { name: "Secciones" });
    await userEvent.click(within(nav).getByRole("button", { name: "Grupos" }));
    expect(await screen.findByText("Crea tu primer grupo")).toBeInTheDocument();
    await userEvent.click(within(nav).getByRole("button", { name: "Preguntar" }));
    expect(await screen.findByRole("textbox", { name: "Pregunta" })).toBeInTheDocument();
    await userEvent.click(within(nav).getByRole("button", { name: "Ajustes" }));
    expect(await screen.findByRole("heading", { name: "Ajustes" })).toBeInTheDocument();
  });

  it("abre la reunión que pide la pastilla (main:navigate)", async () => {
    const { api } = setup();
    await screen.findByRole("button", { name: /Daily del equipo/ });
    const list = await api.meetingsList({});
    await api.mainWindowShow(list.items[0]?.id);
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
  });

  it("permite empezar a grabar a mano y refleja el estado", async () => {
    const { api } = setup();
    const spy = vi.spyOn(api, "sessionStart");
    await userEvent.click(await screen.findByRole("button", { name: "Grabar ahora" }));
    expect(spy).toHaveBeenCalled();
    expect(await screen.findByText(/Grabando/)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Grabar ahora" })).not.toBeInTheDocument());
  });

  it("permite grabar de nuevo mientras una reunión se procesa en segundo plano", async () => {
    const { api, controls } = setup();
    const spy = vi.spyOn(api, "sessionStart");
    await screen.findByRole("button", { name: "Grabar ahora" });
    controls.state.snapshot = { ...controls.state.snapshot, state: { state: "processing", meetingId: "m-old", step: "transcribing" } };
    act(() => controls.emit("session:state", controls.state.snapshot));
    expect(await screen.findByText(/Transcribiendo/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Grabar ahora" }));
    expect(spy).toHaveBeenCalled();
    expect(await screen.findByText(/Grabando/)).toBeInTheDocument();
  });

  it("una cita de otra pantalla abre la reunión", async () => {
    const mock = setup();
    const d = mock.controls.state.meetings[0]!;
    const nav = screen.getByRole("navigation", { name: "Secciones" });
    await userEvent.click(within(nav).getByRole("button", { name: "Preguntar" }));
    const t = await mock.api.chatThreadCreate({ kind: "all" });
    mock.controls.state.messages.push({
      id: "a",
      threadId: t.id,
      role: "assistant",
      content: "Mira [1]",
      citations: [{ n: 1, meetingId: d.meeting.id, meetingTitle: "Daily del equipo", at: 4_500, end: 9_000, snippet: "" }],
      provider: null,
      createdAt: 0,
    });
    await userEvent.click(within(nav).getByRole("button", { name: "Reuniones" }));
    await userEvent.click(within(nav).getByRole("button", { name: "Preguntar" }));
    await userEvent.click(await screen.findByRole("button", { name: "Daily del equipo, 00:04" }));
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
  });

  it("con grupos creados invita a elegir uno", async () => {
    const mock = setup();
    await mock.api.groupCreate("Producto");
    const nav = screen.getByRole("navigation", { name: "Secciones" });
    await userEvent.click(within(nav).getByRole("button", { name: "Grupos" }));
    expect(await screen.findByText("Elige un grupo")).toBeInTheDocument();
  });

  it("se puede parar la grabación desde la barra lateral", async () => {
    const { api } = setup();
    const stop = vi.spyOn(api, "sessionStop");
    const pause = vi.spyOn(api, "sessionPause");
    await userEvent.click(await screen.findByRole("button", { name: "Grabar ahora" }));
    await userEvent.click(await screen.findByRole("button", { name: "Pausar grabación" }));
    expect(pause).toHaveBeenCalled();
    expect(await screen.findByText("En pausa")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Parar grabación" }));
    expect(stop).toHaveBeenCalled();
    expect(await screen.findByText(/Guardando el audio/)).toBeInTheDocument();
  });

  it("⌘K abre la búsqueda rápida y Enter abre la reunión", async () => {
    setup();
    await screen.findByRole("button", { name: /Daily del equipo/ });
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const dialog = await screen.findByRole("dialog", { name: "Búsqueda rápida" });
    await userEvent.type(within(dialog).getByRole("combobox", { name: "Buscar en acta" }), "Daily");
    expect(await within(dialog).findByRole("option", { name: /Daily del equipo/ })).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Búsqueda rápida" })).not.toBeInTheDocument();
  });

  it("la búsqueda rápida encuentra frases del transcript", async () => {
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Búsqueda rápida" }));
    const dialog = await screen.findByRole("dialog", { name: "Búsqueda rápida" });
    await userEvent.type(within(dialog).getByRole("combobox"), "presupuesto");
    await userEvent.click(await within(dialog).findByRole("option", { name: /Yo envío el presupuesto el viernes/ }));
    expect(await screen.findByRole("heading", { name: "Daily del equipo" })).toBeInTheDocument();
  });

  it("la búsqueda rápida navega con las flechas y se cierra con Escape", async () => {
    setup();
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const dialog = await screen.findByRole("dialog", { name: "Búsqueda rápida" });
    await userEvent.type(within(dialog).getByRole("combobox"), "ajustes");
    expect(await within(dialog).findByRole("option", { name: /Abrir Ajustes/ })).toBeInTheDocument();
    await userEvent.keyboard("{ArrowDown}{ArrowUp}{Enter}");
    expect(await screen.findByRole("heading", { name: "Ajustes" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    await userEvent.type(await screen.findByRole("combobox", { name: "Buscar en acta" }), "zzzz");
    expect(await screen.findByText(/Nada coincide/)).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Búsqueda rápida" })).not.toBeInTheDocument();
  });
});
