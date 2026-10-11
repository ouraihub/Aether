@echo off
rem Forward a cargo command after injecting the MSVC environment.
rem
rem Why this wrapper exists: Git ships a coreutils link.exe that sits earlier on
rem PATH than the MSVC linker. Running cargo build directly picks the wrong one
rem and fails with "extra operand ... Try 'link --help'", which gives no hint
rem that the linker itself was the problem.
rem
rem Comments are in English on purpose: cmd.exe reads .bat files in the OEM code
rem page, so UTF-8 Chinese comments turn into garbage and can break parsing.
rem
rem No setlocal: msvc-env.bat must keep its PATH changes in this process.
rem Toolchain policy (portable MSVC locally, runner MSVC only in CI) lives in
rem msvc-env.bat.
rem
rem Usage: scripts\cargo-msvc.bat build --release
rem        scripts\cargo-msvc.bat test -p aether-support

set "MANIFEST=%~dp0..\Cargo.toml"

call "%~dp0msvc-env.bat" || exit /b 1

set "SUBCOMMAND=%1"
shift
set "ARGS="
:collect
if "%1"=="" goto dispatch
set "ARGS=%ARGS% %1"
shift
goto collect

:dispatch
cargo %SUBCOMMAND% --manifest-path "%MANIFEST%"%ARGS%
exit /b %ERRORLEVEL%
