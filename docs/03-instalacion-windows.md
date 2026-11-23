# Instalación en Windows (desarrollo del fork)

En Windows no hay instalador todavía: el empaquetado (MSI/NSIS) es el hito M6. Hoy la app se ejecuta **desde el código fuente**, así que esta página es para quien desarrolla el fork, no para un usuario final. El estado medido del port está en [`win-baseline.md`](./win-baseline.md).

La documentación de instalación de macOS está en [`03-instalacion.md`](./03-instalacion.md).

## 1. Requisitos

| Qué | Versión / detalle |
|---|---|
| Windows | 11 (entorno donde está medido el baseline) |
| Rust | `rustc` y `cargo` ≥ 1.91 (el workspace declara `rust-version = "1.91"` y `edition = "2024"`). El baseline se midió con 1.100.0 |
| Node | 22 es la versión que usa la CI; el baseline se midió con v24.22.1 |
| pnpm | 11 (la CI instala la 11; el baseline usa 11.4.0) |
| WebView2 | Es el runtime con el que Tauri renderiza la UI en Windows. Viene de serie en Windows 11 |
| Git for Windows | Necesario para `scripts/verify.ps1`: solo acepta el Git Bash de Git for Windows (rechaza el `bash` de WSL, que ve otro sistema de ficheros) |
| ripgrep | Necesario para `scripts/guards.sh` (usa `rg`); la CI lo instala con `choco install ripgrep` |
| PowerShell | **5.1 basta** (`powershell.exe`); `verify.ps1` es compatible con 5.1 y con 7, y `pwsh` 7 no se exige |

Versiones de Rust, Node y pnpm: `Cargo.toml:22-23` (workspace), `.github/workflows/ci.yml:31,35` y el entorno medido de [`win-baseline.md`](./win-baseline.md).

## 2. Ejecutar en desarrollo

```powershell
cd apps/desktop
pnpm install --frozen-lockfile
pnpm tauri dev
```

La primera ejecución compila el workspace Rust entero (los crates de `crates/` más `apps/desktop/src-tauri`); tarda. A partir de ahí, `tauri dev` reutiliza la caché de cargo.

Al arrancar se crea el layout de datos (sección 4) y la ventana principal con título `acta`. El componente nativo aparece como no disponible: en el log es normal ver «componente nativo no disponible: plataforma no soportada todavía» (sección 5).

## 3. Verificar el cambio

Desde la raíz del repo:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1
```

El script ejecuta, en orden: un preflight (que `cargo`, `rustc`, `rg`, `pnpm` y un Git Bash real estén en el PATH), `scripts/guards.sh`, `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm install --frozen-lockfile`, `pnpm exec tsc --noEmit` y `pnpm exec vitest run`. Sale con código 1 si algo falla y escribe `verify OK` si todo pasa. Con `-NoInstall` salta el paso de `pnpm install` (útil para reiterar rápido):

```powershell
pwsh -File scripts/verify.ps1 -NoInstall
```

`verify.ps1` **omite deliberadamente** `swift build`: el sidecar Swift (`sidecars/acta-mac`) solo existe en macOS, y la omisión se declara en el resumen del script. Es el equivalente a `scripts/verify.sh` sin ese paso.

## 4. Dónde guarda cosas

```text
%APPDATA%\acta\
  acta.db        acta.db-shm     acta.db-wal
  meetings\<id>\
  models\
  logs\
```

La raíz de datos por defecto sale de `dirs::data_dir()`, que en Windows es `%APPDATA%` (`apps/desktop/src-tauri/src/lib.rs:46`). Para pruebas se puede redirigir con la variable de entorno `ACTA_DATA_DIR` (`apps/desktop/src-tauri/src/system.rs:126`): apúntala **siempre a un directorio temporal**, nunca a datos reales de reuniones.

## 5. Qué no funciona todavía en Windows

Lo que no funciona y qué hito lo cubre está en [`feature-map.md`](./feature-map.md), columna «Estado en Windows» de cada entrada. Resumen: grabación y audio (M2), detección de reunión (M3), transcripción y diarización (M4), nombres de hablantes, capturas de pantalla y permisos (M5).

El estado esperado al arrancar la app en Windows es que el adaptador nativo se degrada a «no disponible»; en el log aparece:

```text
componente nativo no disponible: plataforma no soportada todavía
```

Es el comportamiento diseñado, no un error de arranque.

## 6. Limitaciones conocidas (medidas hoy)

- **No hay instalador de Windows.** El empaquetado es el hito M6; hoy solo se ejecuta en desarrollo.
- **La descarga del modelo local integrado fija una compilación de macOS.** El release fijado es `llama-b12444-bin-macos-arm64.tar.gz` (`crates/acta-llm/src/local/runtime.rs:41`): en Windows no hay selección de release y ese runtime local no arrancará.
- **La detección de proveedores CLI sigue asumiendo macOS.** Intenta lanzar `/bin/zsh` y en Windows registra un aviso con `os error 3`; degrada sin impedir los proveedores por API, pero la autodetección de CLI no funciona todavía.
- **No hay `fsync` de directorio en Windows.** `fsync_dir` es no-op en Windows (abrir un directorio da `Access Denied`), así que un corte de energía puede perder la entrada de un rename ya completado. La recuperación reconcilia `.wav.tmp` y `.wav` huérfanos, pero nadie ha ejecutado todavía la prueba real matando el proceso (hito M2).

## 7. Sin firma

El proyecto no firma en ninguna plataforma: ni Apple ni Windows. En Windows no se añaden certificados ni se configura `signingIdentity`; los guards lo impiden (`scripts/guards.sh` rechaza `codesign` y `signingIdentity: "-"`). El detalle en macOS está en [`03-instalacion.md`](./03-instalacion.md#3-por-qué-no-se-firma).
