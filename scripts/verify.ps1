# Verificacion estatica del workspace en Windows.
#
# Equivalente a scripts/verify.sh, sin `swift build`: el sidecar Swift
# (sidecars/acta-mac) solo se compila en macOS. Compatible con Windows
# PowerShell 5.1 y con PowerShell 7.
#
# Uso:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1
#   pwsh -File scripts/verify.ps1 -NoInstall   # no reinstala dependencias JS
#
# Sale con codigo 1 si algun paso falla.

[CmdletBinding()]
param(
    [switch]$NoInstall
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

$script:failures = New-Object System.Collections.ArrayList
$script:skipped = New-Object System.Collections.ArrayList

function Write-Step {
    param([string]$Text)
    Write-Host ''
    Write-Host "=== $Text ===" -ForegroundColor Cyan
}

function Invoke-Checked {
    param(
        [string]$Name,
        [string]$File,
        [string[]]$Arguments
    )

    Write-Step $Name
    Write-Host ("> {0} {1}" -f $File, ($Arguments -join ' '))

    & $File @Arguments
    $code = $LASTEXITCODE

    if ($code -ne 0) {
        Write-Host ("FALLO: {0} (exit {1})" -f $Name, $code) -ForegroundColor Red
        [void]$script:failures.Add(("{0} (exit {1})" -f $Name, $code))
    }
    else {
        Write-Host ("OK: {0}" -f $Name) -ForegroundColor Green
    }
}

# --- Preflight -------------------------------------------------------------

Write-Step 'preflight'
Write-Host ("repo: {0}" -f $repoRoot)

foreach ($tool in @('cargo', 'rustc', 'rg', 'pnpm')) {
    $cmd = Get-Command $tool -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        Write-Host ("FALLO: falta '{0}' en PATH" -f $tool) -ForegroundColor Red
        [void]$script:failures.Add(("preflight: '{0}' ausente" -f $tool))
    }
    else {
        Write-Host ("OK: {0} -> {1}" -f $tool, $cmd.Source) -ForegroundColor Green
    }
}

# `bash` en Windows puede ser WSL (C:\Windows\System32\bash.exe), que ve otro
# sistema de ficheros y no sirve para scripts/guards.sh. Solo aceptamos Git Bash.
$bash = Get-Command bash -ErrorAction SilentlyContinue
$gitBash = $false
if ($null -ne $bash) {
    $msystem = & bash -c 'printf %s "$MSYSTEM"' 2>$null
    if ($LASTEXITCODE -eq 0 -and $msystem -match '^(MINGW|MSYS|UCRT)') {
        $gitBash = $true
        Write-Host ("OK: git bash ({0}) -> {1}" -f $msystem, $bash.Source) -ForegroundColor Green
    }
    else {
        Write-Host ("AVISO: 'bash' es WSL u otro shell, no Git Bash ({0})" -f $bash.Source) -ForegroundColor Yellow
    }
}
else {
    Write-Host "AVISO: no hay 'bash' en PATH" -ForegroundColor Yellow
}

# --- Guards (fuente unica: scripts/guards.sh) ------------------------------

if ($gitBash) {
    Invoke-Checked -Name 'guards.sh' -File 'bash' -Arguments @('scripts/guards.sh')
}
else {
    Write-Step 'guards.sh'
    Write-Host 'FALLO: scripts/guards.sh necesita Git Bash (no WSL). Instala Git for Windows o añade su bash al PATH.' -ForegroundColor Red
    [void]$script:failures.Add('guards.sh: Git Bash no disponible')
}

# --- Rust -----------------------------------------------------------------

Invoke-Checked -Name 'cargo check --workspace' -File 'cargo' -Arguments @('check', '--workspace')
Invoke-Checked -Name 'cargo clippy --workspace --all-targets' -File 'cargo' -Arguments @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
Invoke-Checked -Name 'cargo test --workspace' -File 'cargo' -Arguments @('test', '--workspace')

# --- Frontend -------------------------------------------------------------

Push-Location -LiteralPath (Join-Path $repoRoot 'apps/desktop')
try {
    if (-not $NoInstall) {
        Invoke-Checked -Name 'pnpm install --frozen-lockfile' -File 'pnpm' -Arguments @('install', '--frozen-lockfile')
    }
    else {
        Write-Step 'pnpm install --frozen-lockfile'
        Write-Host 'OMITIDO (-NoInstall)' -ForegroundColor Yellow
        [void]$script:skipped.Add('pnpm install')
    }

    Invoke-Checked -Name 'tsc --noEmit' -File 'pnpm' -Arguments @('exec', 'tsc', '--noEmit')
    Invoke-Checked -Name 'vitest run' -File 'pnpm' -Arguments @('exec', 'vitest', 'run')
}
finally {
    Pop-Location
}

# --- Swift (solo macOS) ---------------------------------------------------

Write-Step 'swift build'
Write-Host 'OMITIDO: el sidecar Swift solo existe en macOS (este script es para Windows)' -ForegroundColor Yellow
[void]$script:skipped.Add('swift build')

# --- Resumen --------------------------------------------------------------

Write-Step 'resumen'

if ($script:skipped.Count -gt 0) {
    Write-Host ("omitidos: {0}" -f ($script:skipped -join ', ')) -ForegroundColor Yellow
}

if ($script:failures.Count -eq 0) {
    Write-Host 'verify OK' -ForegroundColor Green
    exit 0
}

Write-Host 'verify FALLO:' -ForegroundColor Red
foreach ($failure in $script:failures) {
    Write-Host ("  - {0}" -f $failure) -ForegroundColor Red
}
exit 1
