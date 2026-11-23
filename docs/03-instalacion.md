# Instalación de acta (sin firmar)

acta se compila e instala **localmente**, sin firmar. Todo lo hace `scripts/install-mac.sh`.

Esta página es de macOS. Para ejecutar la app en Windows (desarrollo del fork, sin instalador todavía), ver [`03-instalacion-windows.md`](./03-instalacion-windows.md).

## 1. Requisitos

| Qué | Versión |
|---|---|
| macOS | 14.2 o superior (process tap), Apple Silicon |
| Swift / Xcode CLT | Swift 6 (`swift --version`) |
| Rust | `cargo` y `rustc` (target `aarch64-apple-darwin`) |
| Node + pnpm | pnpm 11 |
| Claude CLI (opcional) | `/opt/homebrew/bin/claude` |

## 2. Instalar

```bash
scripts/install-mac.sh            # -> ~/Applications/acta.app
scripts/install-mac.sh --system   # -> /Applications/acta.app
```

El script, en orden:

1. Comprueba macOS arm64 ≥ 14.2 y las herramientas.
2. Compila el sidecar Swift y lo copia a `apps/desktop/src-tauri/binaries/acta-mac-aarch64-apple-darwin`.
3. `pnpm install --frozen-lockfile`.
4. `tauri build --bundles app --no-sign --features local-embeddings --config src-tauri/tauri.bundle.conf.json` (con búsqueda semántica local).
5. Verifica que existe `Contents/MacOS/acta-mac` y que **no** hay `_CodeSignature`. Si falla, aborta.
6. Cierra acta si está abierta, reemplaza la copia anterior e instala.

**Actualizar** = volver a ejecutar el script.

## 3. Por qué no se firma

acta no se firma ni se notariza: el proyecto no usa un certificado de Apple Developer. El script de instalación además se asegura de que ningún paso del build firme nada, ni siquiera con firma ad hoc.

Evidencia (tauri-cli 2.12.4 depende de `tauri-bundler =2.10.4` y `tauri-macos-sign =2.4.3`):

| Hecho | Dónde |
|---|---|
| La firma solo ocurre si `keychain(...)` devuelve `Some`. Con `signingIdentity: null` y sin `APPLE_CERTIFICATE`, devuelve `None` y **no se firma nada** (no hay fallback ad hoc) | `tauri-bundler/src/bundle/macos/sign.rs:15-43`, `macos/app.rs:115-151` |
| `--no-sign` salta la firma de la app, el sidecar y los frameworks | `tauri-cli/src/build.rs:79-81`, `macos/app.rs:115-116` |
| Las env `APPLE_SIGNING_IDENTITY` y `APPLE_CERTIFICATE` sí activan firma, por eso el script las limpia | `tauri-cli/src/interface/rust.rs:1446-1454`, `sign.rs:18-23` |
| La firma real invoca el binario `codesign` | `tauri-macos-sign/src/keychain.rs:212` |
| Sidecars y frameworks se firman con la app, solo si hay llavero | `macos/app.rs:93-132` |
| El `.dmg` ejecuta un script y puede firmar, por eso solo se pide `--bundles app` | `macos/dmg/mod.rs:205-222` |

La firma ad hoc automática del linker en arm64 (`ld -adhoc_codesign`) no es una invocación de `codesign` y no afecta. Además, el bundler NO reescribe el binario principal en bundles `.app`/`.dmg` (`tauri-bundler/src/bundle.rs:68-75`, "skip patching for macOS-native bundles"), así que esa firma del linker sigue siendo válida y la app arranca en Apple Silicon.

## 4. Primer arranque (Gatekeeper)

Una app local sin firmar no lleva cuarentena, así que normalmente abre directa. Si macOS la bloquea:

- [ ] Intenta abrir acta.
- [ ] Ajustes del Sistema › Privacidad y seguridad › baja hasta "acta fue bloqueada" › **Abrir igualmente**.

En macOS 15+ ya no vale el clic derecho › Abrir. No hace falta quitar atributos con `xattr`.

## 5. Permisos

| Permiso | Para qué | Cómo se concede |
|---|---|---|
| Micrófono | Tu pista | Aviso al empezar |
| Audio del sistema | Pista de los demás (macOS 14.2+) | Aviso al empezar |
| Accesibilidad | Nombres de Teams, hablante activo, ventana de reunión | **Manual**: Ajustes del Sistema › Privacidad y seguridad › Accesibilidad |
| Automatización | URL de Meet en Chrome, Arc, Edge, Brave y Safari | Aviso por navegador |
| Grabación de pantalla | Capturas (opt-in) | Ajustes › Pantalla |

Si un permiso está sin decidir, acta lo pide **antes** de empezar a grabar; si lo deniegas, no se crea la reunión y te dice cuál falta.

Al redactar el primer acta puede salir un diálogo más: «acta.app quiere acceder a los archivos de tu carpeta Documentos». Lo provoca el CLI de Claude que ejecuta acta. El resumen espera hasta que lo contestes (vale cualquiera de las dos respuestas).

TCC asocia los permisos a **acta.app** (el proceso responsable; el sidecar va dentro). En una app sin firmar la identidad es ruta + cdhash, así que recompilar **puede** volver a pedir permisos. Se comprueba en `docs/02-verificacion-manual.md`.

## 6. Configuración inicial

- [ ] **Modelos**: Ajustes › Modelos, descarga `asr`, `diarizer` y `vad` (FluidAudio). El modelo de búsqueda semántica (`multilingual-e5-small`, fastembed) se descarga solo la primera vez que se indexa una reunión, en `~/Library/Application Support/acta/models/fastembed`. Si la red de la empresa lo bloquea, la búsqueda funciona solo por texto y acta reindexa esas reuniones cuando el modelo esté disponible.
- [ ] **Claude CLI**: Ajustes › Proveedores.
  - Ruta absoluta del binario, por ejemplo `/opt/homebrew/bin/claude`.
  - Entorno, solo si usas un directorio de configuración distinto: `CLAUDE_CONFIG_DIR=<tu ruta>`.
  - Si `claude` es un alias de tu `~/.zshrc`, es invisible para `zsh -lc`. "Detectar" ya prueba también una shell interactiva y expande `$HOME`.
  - Elige el esfuerzo y pulsa **Probar**.

## 7. Dónde guarda cosas

```text
~/Library/Application Support/acta/
  acta.db
  logs/acta.log   logs/acta-mac.log
  meetings/<id>/
```

## 8. Desinstalar

```bash
osascript -e 'quit app "acta"'
rm -rf ~/Applications/acta.app            # o /Applications/acta.app
rm -rf ~/Library/Application\ Support/acta   # borra TODAS las reuniones
for s in Microphone SystemPolicyAllFiles Accessibility AppleEvents ScreenCapture; do
  tccutil reset "$s" com.edunavajas.acta
done
```

Solo el micro: `tccutil reset Microphone com.edunavajas.acta`.

## 9. Problemas frecuentes

| Síntoma | Causa probable | Qué hacer |
|---|---|---|
| El script aborta por `_CodeSignature` | Algo ha firmado el bundle | No instales. Revisa env `APPLE_*` y `tauri.bundle.conf.json` |
| `ResourcePathNotFound` al compilar | Falta `binaries/acta-mac-<triple>` | Ejecuta el script entero (compila el sidecar antes) |
| Pista de sistema en ceros | Permiso de Audio del sistema denegado | Concédelo y reinicia la grabación |
| No salen nombres de Teams | Falta Accesibilidad | Actívala a mano y reabre acta |
| No detecta Meet | Falta Automatización | Revisa Privacidad y seguridad › Automatización |
| Pide permisos tras recompilar | Cambia el cdhash | Esperable; ver checklist |
| "Detectar" no encuentra claude | Alias de zsh | Pon la ruta absoluta a mano |
| «Resumiendo…» no termina | Diálogo de la carpeta Documentos sin contestar | Búscalo detrás de las ventanas y contéstalo |
| La voz de los demás sale ininteligible | Auriculares Bluetooth cambiando a manos libres | Arreglado; si vuelve, mira `acta-mac.log` («delivers») |
| El Mac vuelve a bloquear la app | Gatekeeper | "Abrir igualmente" (sección 4) |
