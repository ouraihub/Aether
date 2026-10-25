@echo off
setlocal

rem Remove build output to reclaim disk space; Rust target\ grows to many GB.
rem Keep this file CRLF and ASCII-only: cmd.exe mis-parses LF-only batch files and
rem non-ASCII text in the OEM code page.

for %%R in ("%~dp0..") do set "ROOT=%%~fR"

set "DEEP="
set "YES="
for %%F in (%*) do (
  if /i "%%~F"=="--deep" set "DEEP=1"
  if /i "%%~F"=="-d"     set "DEEP=1"
  if /i "%%~F"=="--yes"  set "YES=1"
  if /i "%%~F"=="-y"     set "YES=1"
  if /i "%%~F"=="--help" goto usage
  if /i "%%~F"=="-h"     goto usage
)

echo ============================================
echo   Aether - clean build output
echo ============================================
echo.
echo Root: %ROOT%
echo.
echo Will remove:
echo   - target\                 Rust build output (data\ folders and junctions kept)
echo   - examples\*\src-tauri\gen  generated Tauri schemas
echo   - .tmp, packages\ui\.svelte-kit, packages\ui\dist
if defined DEEP (
  echo   - node_modules           --deep; run 'pnpm install' afterwards
)
echo.
echo NOT touched: source, docs, git history, Cargo.lock, pnpm-lock.yaml,
echo any data\ folder, or the shared cargo registry in %%USERPROFILE%%\.cargo.
echo.

"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0clean-artifacts.ps1" -Root "%ROOT%" -CheckOnly
if errorlevel 2 goto busy
if errorlevel 1 exit /b 1

if defined YES goto confirmed
choice /c YN /n /m "Proceed? [Y/N] "
if errorlevel 2 goto aborted

:confirmed
set "DEEP_FLAG="
if defined DEEP set "DEEP_FLAG=-Deep"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0clean-artifacts.ps1" -Root "%ROOT%" %DEEP_FLAG%
if errorlevel 2 goto busy
if errorlevel 1 exit /b 1
echo.
echo Done. Next build recompiles from scratch:
echo   node_modules\@ouraihub\msvc-kit\cargo-msvc.bat check --workspace --all-features
goto end

:usage
echo Usage: scripts\clean.bat [--deep] [--yes]
echo.
echo   --deep, -d   also remove node_modules
echo   --yes, -y    skip the confirmation prompt
exit /b 0

:busy
echo ERROR: processes from this repo are holding files; close them first. 1>&2
if not defined YES pause
exit /b 1

:aborted
echo Aborted. Nothing was removed.
if not defined YES pause
exit /b 1

:end
echo.
if not defined YES pause
exit /b 0
