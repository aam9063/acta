# Baseline de `acta` en Windows (M0)

Estado observado del fork el **2026-11-23** sobre el commit `9242e2e` (acta 0.1.2), que es idéntico a `upstream/main` (`rev-list --left-right --count main...upstream/main` → `0 0`).

Todo lo de este documento se ejecutó en Windows; cada línea es salida real, no una deducción leyendo código. Lo que no se pudo observar se dice explícitamente.

## Entorno

| Componente | Versión / valor |
|---|---|
| Sistema | Windows 11, shell MSYS (`MINGW64_NT-10.0-26300`) |
| Target de Rust | `x86_64-pc-windows-msvc` (host por defecto, sin `--target`) |
| cargo / rustc | 1.100.0 |
| node / pnpm | v24.22.1 / 11.4.0 |
| ripgrep | 15.2.0 |
| PowerShell | 5.1 (`powershell.exe`; **no** hay `pwsh` 7 en la máquina) |
| Git Bash | `C:\Program Files\Git\usr\bin\bash.exe`, `MSYSTEM=MINGW64` |
| gentle-ai | 5.0.0 |

## Resultado de una pasada completa

```
=== preflight ===
OK: cargo -> C:\Users\alber\.cargo\bin\cargo.exe
OK: rustc -> C:\Users\alber\.cargo\bin\rustc.exe
OK: rg -> ...\rg.exe
OK: git bash (MINGW64) -> C:\Program Files\Git\usr\bin\bash.exe
=== guards.sh ===
OK: guards.sh
=== cargo check --workspace ===
FALLO: cargo check --workspace (exit 101)
=== cargo clippy --workspace --all-targets ===
FALLO: cargo clippy --workspace --all-targets (exit 101)
=== cargo test --workspace ===
FALLO: cargo test --workspace (exit 101)
=== tsc --noEmit ===
OK: tsc --noEmit
=== vitest run ===
 Test Files  39 passed (39)
      Tests  533 passed (533)
OK: vitest run
=== swift build ===
OMITIDO: el sidecar Swift solo existe en macOS
```

Comando: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1 -NoInstall`, con `pnpm install --frozen-lockfile` verificado aparte (**exit 0**).

## Lo que ya funciona en Windows

- `scripts/guards.sh` corre en Git Bash y sale 0 (`guards OK`): todas las restricciones duras de `crates/` y `apps/desktop/src-tauri/src` se cumplen.
- `pnpm install --frozen-lockfile` instala sin tocar el lockfile (`@tauri-apps/api` 2.12.4, `@tauri-apps/cli` 2.12.4).
- `pnpm exec tsc --noEmit` pasa sin errores.
- `pnpm exec vitest run` pasa: **39 ficheros, 533 tests**.

Conclusión: el frontend del port no tiene deuda pendiente; el trabajo de M1 es exclusivamente Rust y empaquetado.

## Lo que no funciona: los dos muros reales

### Muro 1 — `crates/acta-llm/src/local/runtime.rs` (no está en la tabla de bloqueos del manual)

`cargo check --workspace` sale con **exit 101**. Tres errores, todos en el mismo fichero:

```
error[E0433]: cannot find `unix` in `os`
  --> crates\acta-llm\src\local\runtime.rs:99:18
 99 |     use std::os::unix::fs::PermissionsExt;

error[E0433]: cannot find `unix` in `os`
   --> crates\acta-llm\src\local\runtime.rs:117:22
117 |             std::os::unix::fs::symlink(target, &out)?;

error[E0599]: no associated function or constant named `from_mode`
   --> crates\acta-llm\src\local\runtime.rs:122:53
122 |             f.set_permissions(std::fs::Permissions::from_mode(mode))?;
```

Es un bloque de preparación de un directorio de runtime local que usa permisos POSIX y enlaces simbólicos de tipo Unix. Es el **primer** muro y tapa el resto: nada que dependa de `acta-llm` llega a compilarse. `acta-desktop` depende de él, así que la app nunca se enlaza.

### Muro 2 — el build script de `acta-desktop` (blocker #3 del manual, confirmado)

Con `cargo check --workspace --keep-going` aparece un fallo independiente del anterior:

```
error: failed to run custom build command for `acta-desktop v0.1.2 (…\apps\desktop\src-tauri)`
Caused by:
  process didn't exit successfully: `…\build_script_build` (exit code: 1)
  --- stdout
  …
  package.metadata does not exist
  `…\apps\desktop\src-tauri\icons/icon.ico` not found; required for generating a Windows Resource file during tauri-build
```

`apps/desktop/src-tauri/icons/` contiene `128x128.png`, `128x128@2x.png`, `32x32.png`, `icon.icns`, `icon.png`. No hay `icon.ico`, y `tauri-build` lo exige en Windows para generar el fichero de recursos.

### Muro 3 — `pnpm tauri dev`

```
tauri_dev_exit=101
error: could not compile `acta-llm` (lib) due to 3 previous errors
[ELIFECYCLE] Command failed with exit code 101.
```

No hay ventana: `tauri dev` no llega a arrancar porque el binario no compila. `pnpm install` sí termina bien (exit 0). En esta ejecución no aparece el muro 2 porque `cargo` sin `--keep-going` aborta en el primer crate que falla.

## Estado de los ocho bloqueos de §6 del manual

| # | Hipótesis del manual | Estado observado |
|---|---|---|
| 1 | `system.rs::disk_free` usa `std::os::unix` y `libc::statvfs` | **Presente en el código, no observado fallar.** `system.rs:8-22`, con `use std::os::unix::ffi::OsStrExt` en `:9` y `MaybeUninit::<libc::statvfs>` en `:11`; `libc` solo se usa ahí (`Cargo.toml:52`, `apps/desktop/src-tauri/Cargo.toml:39`). La compilación muere antes en el muro 1, así que el error no se ha podido ver. |
| 2 | `keyring` solo tiene la feature `apple-native` | **Confirmado leyendo el manifiesto.** `Cargo.toml:79`. Su único consumidor es `crates/acta-store/src/secrets/mod.rs:47-80`, sin ningún `cfg` de plataforma: la superficie a tocar en Windows es esa feature y ese fichero. |
| 3 | Falta `icon.ico` | **Confirmado compilando.** Ver muro 2. |
| 4 | `tauri.conf.json` tiene claves de macOS | **Confirmado leyendo.** `"macOSPrivateApi": true` (`:13`), `titleBarStyle` (`:19`), `trafficLightPosition` (`:21-24`), dos bloques `windowEffects` (`:25-32`, `:48-55`), scope `$HOME/Library/Application Support/acta/**` (`:66-71`), `targets: ["app"]` (`:74-77`). El test que fija el scope es `apps/desktop/src-tauri/tests/wave5.rs:326-333`. No existe `tauri.windows.conf.json`. |
| 5 | `tauri.bundle.conf.json` es de macOS | **Confirmado leyendo.** Contiene `externalBin: ["binaries/acta-mac"]`, firma y `hardenedRuntime`. |
| 6 | `crates/acta-core/src/layout.rs` resuelve la ruta de datos | **Refutado: el manual apunta al fichero equivocado.** `layout.rs` es lógica pura de rutas. La resolución real está en `apps/desktop/src-tauri/src/lib.rs:45-47` (`system::data_root(std::env::var_os(DATA_DIR_ENV), dirs::data_dir())`), con `ACTA_DATA_DIR` en `system.rs:114` y tests en `system.rs:142-148`. Lo único incorrecto son cuatro comentarios: `layout.rs:3`, `layout.rs:40`, `lib.rs:4`, `lib.rs:44`. |
| 7 | `scripts/` y CI son de macOS | **Confirmado leyendo.** `ci.yml` tiene un solo job en `macos-26` y hace `brew install ripgrep` porque `guards.sh` necesita `rg`. Mitad resuelta por este hito con `scripts/verify.ps1`. |
| 8 | `libc` condicionada a unix | **Confirmado**: solo se usa en `disk_free`. No se ha tocado. |

Bloqueo **nuevo**, ausente en el manual: muro 1 (`acta-llm`).

## Lo que este baseline NO prueba

- Que `disk_free` sea el siguiente error tras arreglar `acta-llm`: es hipótesis, no observación.
- Nada de macOS: la CI de macOS no se ejecutó en esta máquina. Los cambios de este hito son añadidos (un script nuevo y documentación), así que no alteran el comportamiento en macOS, pero eso hay que confirmarlo en la CI, no aquí.
- Nada de la UI en marcha: no hay binario, por tanto no hay ventana, ni WebView2, ni overlay.

## Cómo reproducirlo

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1
```

O paso a paso, si se prefiere:

```powershell
bash scripts/guards.sh
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop
pnpm install --frozen-lockfile
pnpm exec tsc --noEmit
pnpm exec vitest run
```

`scripts/verify.ps1` no ejecuta `swift build`: el sidecar `sidecars/acta-mac` solo existe en macOS.
