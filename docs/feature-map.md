# Feature Map

Mapa de funcionalidades de `acta` para convertir un reporte vago («la pastilla no sale») en una ruta de reproducción concreta. Cada entrada dice dónde está el código, cómo se llega desde la UI, cómo se controla por programa y cómo está en Windows.

Estado medido el 2026-11-23 sobre el commit `9242e2e`: ver `docs/win-baseline.md`.

**Si no encontrar algo rápido con este mapa, el mapa tiene un defecto: arréglalo en el mismo PR.**

Convenciones de la columna «Cómo llegar»: la app no usa router; las vistas son `useState<View>` en `apps/desktop/src/app/App.tsx` (`View = "meetings"|"groups"|"ask"|"settings"|"live"`) y se navega por la barra lateral (`app/Sidebar.tsx`). «Ajustes → X» significa: barra lateral → «Ajustes» → sección X, con la nav `aria-label="Secciones de ajustes"` (`features/settings/SettingsContainer.tsx:188`).

---

## GRABACIÓN (GRABAR, PAUSAR, PARAR, AUTOPARADA)

- **Qué es**: Grabar una reunión en curso en dos pistas separadas y pararla sola cuando ya no habla nadie.
- **Subfuncionalidades**: grabar ahora; pausar y reanudar; parar; indicadores de nivel «Yo»/«Otros»; pregunta de autoparada por silencio; descartar la reunión detectada.
- **Cómo llegar en la UI**: barra lateral → botón «Grabar ahora» (`app/Sidebar.tsx:39-43`); con una grabación activa, la tarjeta de la barra lateral muestra «Grabando» o «En pausa” y los medidores «Yo»/«Otros» (`app/Sidebar.tsx:69-92`); pestaña «En directo» (`features/live/LiveView.tsx:23`); configuración en Ajustes → «Grabación y autoparada» (`features/settings/GeneralSections.tsx:169-196`).
- **Atajos de teclado**: ninguno. No hay `global-shortcut`: el único plugin registrado es `tauri-plugin-opener` (`apps/desktop/src-tauri/src/lib.rs:170`).
- **Selectores/ids para control programático**: `aria-label="Parar grabación"` (`app/Sidebar.tsx:84`), `aria-label` de pausar en `:80`, `role="status"` en `:70`. Los tests consultan por texto y rol (`app/App.test.tsx`).
- **Ubicación del código**: `crates/acta-core/src/recording/`, `crates/acta-audio/` (segmentos, recuperación, supervisor), UI en `apps/desktop/src/app/Sidebar.tsx` y `apps/desktop/src/features/live/`.
- **Puertos/backend implicados**: `AudioSource` (`crates/acta-core/src/recording/ports.rs:15`). Comandos `session_start`, `session_pause`, `session_resume`, `session_stop`, `session_dismiss`, `prompt_answer` (`api/commands.rs:75-81`). Eventos `session:state`, `session:levels`, `session:health` (`api/events.rs:24-26`).
- **Estado en Windows**: **no soportado (M2)**. El único adaptador es `crates/acta-mac-bridge/src/audio/mod.rs`.
- **Trampas conocidas**: el silencio que dispara la autoparada se mide con «Umbral de silencio (dBFS)» (`GeneralSections.tsx:193`) y «Minutos de silencio antes de preguntar» (`:172`); si el umbral no se cumple nunca, la app no pregunta. Las dos pistas son contratos distintos: `mic` es «Yo», `system` son los demás.

## DETECCIÓN DE REUNIÓN

- **Qué es**: Avisar de que empezó una reunión y ofrecer grabarla sin que el usuario busque el botón.
- **Subfuncionalidades**: detección automática; lista de apps vigiladas; lista de apps ignoradas; aviso en la pastilla flotante y en la bandeja del sistema.
- **Cómo llegar en la UI**: Ajustes → «Detección» (`features/settings/GeneralSections.tsx:266-309`); el aviso aparece en `app/Sidebar.tsx:96-104` y en la bandeja (`apps/desktop/src-tauri/src/tray.rs:40-42`, «Reunión en …» / «Grabar» / «Ignorar»).
- **Atajos de teclado**: ninguno.
- **Selectores/ids para control programático**: el onboarding usa `getByRole("checkbox", { name: "Avisarme cuando empiece una reunión" })` (`onboarding/Onboarding.test.tsx:229`); no hay `data-testid`.
- **Ubicación del código**: `crates/acta-core/src/detection/`, `apps/desktop/src/features/settings/GeneralSections.tsx`.
- **Puertos/backend implicados**: `MeetingDetector` (`crates/acta-core/src/detection/ports.rs:8`), alimentado a su vez por `SpeakerNameSource`. Comandos `session_get_state`, `prompt_answer`, `settings_update`.
- **Estado en Windows**: **no soportado (M3)**.
- **Trampas conocidas**: en macOS el detector se apoya en el uso del micrófono por otras apps y en los nombres de ventana; el aviso puede dispararse por una app que no está en la lista de «Apps de reunión». Ignorar una app es la salida, no desactivar la detección entera.

## NOMBRES DE HABLANTES

- **Qué es**: Poner nombre a cada voz de la transcripción y recordarla para la próxima reunión.
- **Subfuncionalidades**: renombrar hablante; lista de «Voces recordadas»; borrar una voz recordada; umbral de similitud de voz.
- **Cómo llegar en la UI**: transcripción de una reunión → botones «Renombrar a Ana», «Guardar nombre», «Confirmar a Ana» (`features/transcript/TranscriptTab.tsx`, tests en `transcript/Transcript.test.tsx:43-71`); Ajustes → «Voces recordadas» (`features/settings/GeneralSections.tsx:87,103`, estado vacío «Todavía no recuerdo ninguna voz»); avanzado → «Similitud mínima de voz (%)» (`:146`).
- **Atajos de teclado**: ninguno.
- **Selectores/ids para control programático**: ninguno propio; los tests usan texto y rol.
- **Ubicación del código**: `crates/acta-core/src/detection/`, `apps/desktop/src/features/transcript/`.
- **Puertos/backend implicados**: `SpeakerNameSource` (`crates/acta-core/src/detection/ports.rs:18`). Comandos `speaker_rename`, `voiceprints_list`, `voiceprint_delete` (`api/commands.rs:88,107-108`).
- **Estado en Windows**: **no soportado (M5)**. El adaptador macOS usa AXUIElement sobre Teams (`crates/acta-mac-bridge/src/speakers/mod.rs`).
- **Trampas conocidas**: en macOS depende de Accesibilidad concedida y de que Teams exponga los nombres por accesibilidad; si el nombre no llega, la UI no falla, simplemente deja «Hablante N».

## TRANSCRIPCIÓN Y DIARIZACIÓN

- **Qué es**: Convertir el audio en texto con hablantes separados, en diferido y en directo.
- **Subfuncionalidades**: transcripción por reunión; separación de hablantes; transcripción en vivo; descarga y estado de modelos; reprocesar una reunión; buscar dentro de una transcripción.
- **Cómo llegar en la UI**: detalle de reunión → pestañas «Transcripción» (`features/transcript/TranscriptTab.tsx:63`, vacío «Sin transcripción»), «Escaleta» (`features/escaleta/EscaletaTab.tsx:46`), «Línea de tiempo» (`features/timeline/TimelineTab.tsx:36`); vista «En directo» (`features/live/LiveView.tsx`); Ajustes → interruptores «Separar hablantes» (`GeneralSections.tsx:72`) y «Transcripción en vivo» (`:73`); modelos en Ajustes → «Modelos» (`features/settings/ModelsSection.tsx:13-15`).
- **Atajos de teclado**: `Ctrl+F` / `⌘F` abre la búsqueda dentro de transcripción y vista en vivo (`shared/useLineSearch.ts:121-128`); `Enter` va a la siguiente coincidencia (`shared/ui/FindBar.tsx:33-37`).
- **Selectores/ids para control programático**: por texto; p. ej. `getByText("Empezamos con el plan.")` (`transcript/Transcript.test.tsx:25`) y «Siguiente coincidencia» (`:135`).
- **Ubicación del código**: `crates/acta-core/src/transcription/`, `apps/desktop/src/features/transcript/`, `features/live/`.
- **Puertos/backend implicados**: `Transcriber` (`transcription/ports.rs:14`), `Diarizer` (`:24`), `LiveTranscriber` (`:36`). Comandos `meeting_reprocess`, `meeting_search_transcript`, `live_transcript_get`, `models_status`, `models_download`. Eventos `live:partial`, `live:transcript`, `processing:progress` (`api/events.rs:27,31,36`).
- **Estado en Windows**: **no soportado (M4)**. El adaptador macOS delega en FluidAudio del sidecar Swift (`acta-mac-bridge/src/transcription/mod.rs`).
- **Trampas conocidas**: la transcripción en vivo está desactivada por defecto y su estado vacío remite a Ajustes → Grabación; si el texto no aparece, primero mira ese interruptor y el estado de los modelos.

## CAPTURAS DE PANTALLA

- **Qué es**: Guardar imágenes de la pantalla compartida para no perder lo que se enseñó en la reunión.
- **Subfuncionalidades**: captura automática con palabras clave; captura manual; galería con navegación por teclado; borrar una captura.
- **Cómo llegar en la UI**: detalle de reunión → pestaña «Capturas» (`features/captures/CapturesTab.tsx:39`; vacío: «Se guardan si activas las capturas de la pantalla compartida en Ajustes. Están desactivadas por defecto.»); Ajustes → «Capturas» (`GeneralSections.tsx:325-332`: «Capturar la pantalla compartida» en `:327`, «Palabras que fuerzan una captura» en `:330`).
- **Atajos de teclado**: `←` / `→` dentro del visor de capturas (`CapturesTab.tsx:31-32`). Ninguno global.
- **Selectores/ids para control programático**: por texto (`features/captures/Captures.test.tsx`).
- **Ubicación del código**: `crates/acta-core/src/screen/`, `apps/desktop/src/features/captures/`.
- **Puertos/backend implicados**: `ScreenCapturer` (`crates/acta-core/src/screen/mod.rs:106`). Comandos `capture_manual`, `capture_delete`; evento `capture:saved`.
- **Estado en Windows**: **no soportado (M5)**. El adaptador macOS usa ScreenCaptureKit (`acta-mac-bridge/src/screen/mod.rs`).
- **Trampas conocidas**: desactivadas por defecto; sin la opción activada la pestaña existe pero siempre está vacía, y eso no es un bug.

## ACTA, CHAT Y BÚSQUEDA

- **Qué es**: Convertir una reunión en acta con tareas, preguntarle cosas a las notas y buscar en toda la biblioteca.
- **Subfuncionalidades**: acta generada con tareas «Mis tareas»/«Tareas de otros»; chat por reunión con historial; preguntar a toda la biblioteca; paleta de búsqueda rápida; proveedores LLM (remotos y locales).
- **Cómo llegar en la UI**: detalle de reunión → «Acta» (`features/minutes/MinutesTab.tsx:81` vacío «Todavía no hay acta»; listas en `:124-125`); chat (`features/chat/ChatView.tsx`: «Enviar», «Detener», «Reintentar», «Copiar respuesta»); vista «Preguntar» (`features/chat/AskPage.tsx:16`); paleta con `Ctrl+K`/`⌘K` (`app/App.tsx:93-100`); proveedores en Ajustes → «Proveedores de IA» (`features/providers/ProvidersSection.tsx`).
- **Atajos de teclado**: `Ctrl+K`/`⌘K` abre y cierra la paleta (`app/App.tsx:93-100`); `⌘D` marca favorito (`app/CommandPalette.tsx:140`); en la paleta `↓`/`↑`/`Enter`/`Escape` (`:131-147`); en el chat `Enter` envía, `Shift+Enter` hace salto de línea (`ChatView.tsx:235`).
- **Selectores/ids para control programático**: `role="dialog"` + `aria-label="Búsqueda rápida"` (`app/CommandPalette.tsx:156`), `aria-label="Buscar en acta"` en el input (`:162`), `role="listbox"` + `aria-label="Resultados"` (`:178`). Único `data-testid` de `src/`: `data-testid="scroller"` (`chat/Chat.test.tsx:193`).
- **Ubicación del código**: `crates/acta-llm/` (proveedores, prompts, acta, chat), `crates/acta-store/` (búsqueda, hilos, secretos), `apps/desktop/src/features/{minutes,chat,providers}/`, `app/CommandPalette.tsx`.
- **Puertos/backend implicados**: `LlmProvider` (`llm/ports.rs:8`), `LlmProviderFactory` (`:17`), `Retriever` (`chat/mod.rs:106`). Comandos `chat_send`, `chat_cancel`, `chat_thread_*`, `chat_messages_list`, `search`, `llm_*`, `secret_set`, `secret_delete`. Eventos `chat:delta`, `live:answer`.
- **Estado en Windows**: **portable, a verificar en M1**. Ningún proveedor LLM viene activado de fábrica.
- **Trampas conocidas**: las claves de proveedor se guardan en el llavero del sistema (`crates/acta-store/src/secrets/mod.rs:47-80`, sin `cfg` de plataforma), así que en Windows dependen de la feature `windows-native` de `keyring` (bloqueo 2 del baseline). Sin llavero, `secret_set` falla con `Unavailable` y el chat no funciona aunque el proveedor esté configurado.

## BIBLIOTECA Y GRUPOS

- **Qué es**: Ver todas las reuniones guardadas, agruparlas por proyecto y limpiar las que sobran.
- **Subfuncionalidades**: listar y abrir reuniones; favoritos; crear grupos; sugerir reuniones para un grupo; añadir y quitar reuniones de un grupo; sugerencias de limpieza; exportar transcripción y audio; abrir la carpeta de la reunión.
- **Cómo llegar en la UI**: vista principal «Reuniones» (`features/meetings/MeetingList.tsx`, `MeetingRow.tsx`; favorito «Marcar como favorito»); barra lateral → «Grupos» (`features/groups/GroupsView.tsx:98`, «Crear grupo», «Buscar sugerencias», «Añadir reuniones»); banner de limpieza `features/cleanup/CleanupBanner.tsx` y hoja `CleanupSheet.tsx`.
- **Atajos de teclado**: solo los de la paleta (`Ctrl+K`/`⌘K` lista reuniones).
- **Selectores/ids para control programático**: por texto y rol; p. ej. `getByRole("button", { name: "Crear grupo" })` (`groups/Groups.test.tsx:24`). Las filas no tienen `data-testid` ni `aria-label`: **es una carencia, no un olvido documentado**.
- **Ubicación del código**: `crates/acta-store/`, `apps/desktop/src/features/{meetings,groups,cleanup}/`.
- **Puertos/backend implicados**: `MeetingStore` (`library/ports.rs:17`), `GroupStore` (`:68`), `LlmProvider` para títulos sugeridos (`groups_suggest_title`). Comandos `meetings_list`, `meeting_get`, `meeting_update`, `meeting_delete`, `meeting_set_favourite`, `meeting_retry`, `meeting_export_transcript`, `meeting_export_audio`, `open_meeting_folder`, `groups_*`, `group_*`, `cleanup_*`. Evento `meeting:updated`.
- **Estado en Windows**: **portable, a verificar en M1** (SQLite con rusqlite bundled, FTS5 y sqlite-vec).
- **Trampas conocidas**: `open_meeting_folder` abre el explorador de Windows con la ruta de datos de `ACTA_DATA_DIR` o `%APPDATA%`; si apuntaste a un directorio temporal para probar, la carpeta que abre es la temporal.

## PASTILLA FLOTANTE (OVERLAY)

- **Qué es**: Una ventana pequeña siempre visible para grabar sin cambiar de aplicación.
- **Subfuncionalidades**: aviso de reunión detectada con «Grabar»/«Ignorar»; medidores «Yo»/«Otros»; pausar, reanudar, parar; pregunta de autoparada; «Acta lista»/«Abrir acta»; estado de error con `role="alert"`; expandir y contraer.
- **Cómo llegar en la UI**: es una segunda ventana, `<raíz>/apps/desktop/overlay.html` → entrada `apps/desktop/src/overlay/main.tsx` → `overlay/Overlay.tsx` y `features/overlay/OverlayPill.tsx` (`OverlayContainer.tsx` la envuelve). No se llega desde la ventana principal: aparece sola según el estado de la sesión.
- **Atajos de teclado**: ninguno.
- **Selectores/ids para control programático**: `aria-label` en los botones de pausar y parar (`OverlayPill.tsx:135,141`), `role="meter"` (`:37`), `role="group"` (`:76`); tests por texto (`overlay/Overlay.test.tsx:180-182`).
- **Ubicación del código**: `apps/desktop/src-tauri/src/overlay.rs`, `apps/desktop/src/features/overlay/`, `apps/desktop/overlay.html`.
- **Puertos/backend implicados**: ninguno propio; espeja el estado de sesión. Comandos `overlay_set_expanded` (`src-tauri/src/commands.rs:124-138`) y `main_window_show`; evento `main:navigate`.
- **Estado en Windows**: **parcial (M1)**. La rama no macOS ya existe: `set_overlay` en `apps/desktop/src-tauri/src/lib.rs:97-103` muestra y oculta una ventana normal en lugar de un NSPanel (`overlay.rs:77-99` es la conversión a NSPanel, solo macOS). Falta comprobar que la ventana queda siempre encima y no roba el foco.
- **Trampas conocidas**: si la pastilla no aparece en macOS suele ser el NSPanel; en Windows será la regla de visibilidad (`overlay.rs:72-74`) o el flag de siempre encima, no la conversión a panel.

## PERMISOS

- **Qué es**: Ver y pedir los permisos del sistema que la grabación necesita.
- **Subfuncionalidades**: estado por permiso; botón para pedir; atajo al panel del sistema; onboarding de permisos.
- **Cómo llegar en la UI**: Ajustes → «Permisos» (`features/settings/PermissionsSection.tsx:65`, etiquetas «Micrófono», «Audio del sistema», «Grabación de pantalla», «Accesibilidad», «Automatización» en `:13-19`); onboarding, paso «Permisos de macOS» (`features/onboarding/steps.tsx:64`, botón «Abre el panel de Privacidad y seguridad» en `:92`).
- **Atajos de teclado**: ninguno.
- **Selectores/ids para control programático**: por texto (`settings/Settings.test.tsx`, `onboarding/Onboarding.test.tsx`).
- **Ubicación del código**: `crates/acta-core/src/permissions/`, `apps/desktop/src/features/settings/PermissionsSection.tsx`, `features/onboarding/steps.tsx`.
- **Puertos/backend implicados**: `PermissionsProbe` (`crates/acta-core/src/permissions/mod.rs:43-46`). Comandos `permissions_status`, `permissions_request`, `open_system_settings`; evento `permissions:changed`.
- **Estado en Windows**: **no soportado (M5) pero degradado, no roto**. En Windows no hay TCC: la sonda de nivel de app ya devuelve un valor por defecto fuera de macOS (`apps/desktop/src-tauri/src/system.rs:48-51`). Lo que falta es la privacidad de micrófono de Windows y el texto, que hoy es específico de macOS.
- **Trampas conocidas**: el texto de la sección y del onboarding es copia de macOS («Permisos de macOS», «Abre el panel de Privacidad y seguridad»). Si en Windows sigue apareciendo tal cual, es copy pendiente, no una feature rota.

## AJUSTES Y ONBOARDING

- **Qué es**: Configurar la app y guiar el primer arranque.
- **Subfuncionalidades**: nueve secciones de ajustes; onboarding de cinco pasos; repetir la bienvenida; elegir entrada de audio; abrir la carpeta de datos; perfil del sistema.
- **Cómo llegar en la UI**: barra lateral → «Ajustes» (`app/Sidebar.tsx:16-21`, click en `:129-133`); dentro, `<h1>Ajustes</h1>` y nav `aria-label="Secciones de ajustes"` (`features/settings/SettingsContainer.tsx:187-188`) con «General», «Grabación y autoparada», «Detección», «Capturas», «Proveedores de IA», «Modelos», «Permisos», «Acerca de» (`:18-28`). Onboarding: aparece una sola vez si `settings.onboarding.completed` es falso (`app/App.tsx:41-47`); se repite desde la fila «Bienvenida» (`GeneralSections.tsx:135`). Pasos: bienvenida, permisos, transcripción, detección y autoparada, fin (`features/onboarding/steps.tsx:30,64,134,205,257`).
- **Atajos de teclado**: ninguno.
- **Selectores/ids para control programático**: `aria-label` en varios inputs: «Tu nombre» (`GeneralSections.tsx:51`), «URLs de Meet» (`:309`), «Nombre de ${p.label}» (`ProvidersSection.tsx:110`); los tests de ajustes van por texto (`settings/Settings.test.tsx:31-35`).
- **Ubicación del código**: `apps/desktop/src/features/settings/`, `apps/desktop/src/features/onboarding/`, `apps/desktop/src/app/App.tsx`.
- **Puertos/backend implicados**: `PermissionsProbe` (sección de permisos) y la fábrica de `LlmProvider` (sección de proveedores). Comandos `settings_get`, `settings_update`, `app_info`, `apps_list`, `system_profile`, `open_data_folder`, `audio_inputs_list`.
- **Estado en Windows**: **portable, a verificar en M1**. La sección de permisos y el paso de permisos del onboarding son específicos de macOS.
- **Trampas conocidas**: no hay router; si buscas una URL de ajustes, no existe. El onboarding puede tapar la UI en el primer arranque de una instalación limpia, y con `ACTA_DATA_DIR` apuntando a un directorio temporal cada arranque cuenta como primer arranque.
