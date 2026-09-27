@echo off
rem Beamer's local llama.cpp model server for K2-Horizon extraction
rem (Windows-on-ARM64, CPU-only). Beamer does NOT spawn this directly; it
rem runs the Scheduled Task that runs this script (src/components/, only once
rem the runtime and a verified model are all on disk) and the task's own
rem AtLogOn trigger covers every login after that. Beamer must degrade
rem gracefully when this is absent or the model file is missing: the note is
rem still captured, it just is not extracted.
rem
rem Embedded in beamer.exe and written out by it (src/components/mod.rs); edit
rem it here and a new build rewrites it on every install.
rem
rem %~dp0 resolves to this script's own directory (the fixed runtime path,
rem %LOCALAPPDATA%\Beamer\llama-k2horizon\) so this file is portable and
rem carries no machine-specific path. The model
rem lives at a DIFFERENT fixed path (%USERPROFILE%\models\beamer\), resolved
rem explicitly below rather than relative to %~dp0.
cd /d "%~dp0"
llama-server.exe --models-dir "%USERPROFILE%\models\beamer" --models-preset "%~dp0llama-models-bearcave.ini" --models-max 1 --host 127.0.0.1 --port 8080
