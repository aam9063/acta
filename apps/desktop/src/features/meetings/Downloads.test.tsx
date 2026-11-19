import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { createMockApi } from "../../api";
import { ApiProvider } from "../../app/ApiContext";
import { Sidebar } from "../../app/Sidebar";
import { SettingsContainer } from "../settings/SettingsContainer";
import { MeetingDetailContainer } from "./MeetingDetailContainer";

function detail(withAudio = true) {
  const { api, controls } = createMockApi();
  const d = controls.state.meetings[0]!;
  d.audio = withAudio ? { mic: "/m/mic.wav", system: "/m/system.wav", mix: "/m/mix.wav" } : { mic: null, system: null, mix: null };
  render(
    <ApiProvider api={api}>
      <MeetingDetailContainer meetingId={d.meeting.id} onBack={() => {}} onOpenMeeting={() => {}} />
    </ApiProvider>,
  );
  return { api, controls, d };
}

describe("descargas de la reunión", () => {
  it("descarga la transcripción en texto y en markdown y avisa de dónde queda", async () => {
    const { controls, d } = detail();
    await userEvent.click(await screen.findByRole("button", { name: "Descargar" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Transcripción \(\.txt\)/ }));
    await waitFor(() => expect(controls.state.exports).toEqual([{ kind: "transcript", meetingId: d.meeting.id, format: "txt" }]));
    expect(await screen.findByRole("status")).toHaveTextContent(/Descargas/);
    await userEvent.click(screen.getByRole("button", { name: "Descargar" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Transcripción \(\.md\)/ }));
    await waitFor(() => expect(controls.state.exports[1]).toEqual({ kind: "transcript", meetingId: d.meeting.id, format: "md" }));
  });

  it("descarga el audio y abre la carpeta de la reunión", async () => {
    const { controls, d } = detail();
    await userEvent.click(await screen.findByRole("button", { name: "Descargar" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Audio/ }));
    await waitFor(() => expect(controls.state.exports).toEqual([{ kind: "audio", meetingId: d.meeting.id }]));
    await userEvent.click(screen.getByRole("button", { name: "Descargar" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Abrir carpeta de la reunión" }));
    await waitFor(() => expect(controls.state.folders).toEqual([`meeting:${d.meeting.id}`]));
  });

  it("sin audio, la opción de audio está desactivada", async () => {
    detail(false);
    await userEvent.click(await screen.findByRole("button", { name: "Descargar" }));
    expect(screen.getByRole("menuitem", { name: /Audio/ })).toBeDisabled();
  });

  it("si la descarga falla, lo cuenta", async () => {
    const { api } = detail();
    api.meetingExportAudio = () => Promise.reject({ code: "notFound", message: "Esta reunión no tiene audio disponible." });
    await userEvent.click(await screen.findByRole("button", { name: "Descargar" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Audio/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no tiene audio");
  });
});

describe("carpeta de datos y versión", () => {
  it("Ajustes abre la carpeta de datos", async () => {
    const { api, controls } = createMockApi();
    render(
      <ApiProvider api={api}>
        <SettingsContainer />
      </ApiProvider>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /Acerca de/ }));
    await userEvent.click(await screen.findByRole("button", { name: "Abrir carpeta de datos" }));
    await waitFor(() => expect(controls.state.folders).toEqual(["data"]));
  });

  it("la barra lateral enseña la versión que da la app", async () => {
    const { api } = createMockApi();
    render(
      <ApiProvider api={api}>
        <Sidebar view="meetings" onView={() => {}} onSearch={() => {}} />
      </ApiProvider>,
    );
    expect(await screen.findByText("v0.1.0-mock")).toBeInTheDocument();
  });
});
