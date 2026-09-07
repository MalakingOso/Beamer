@echo off
rem Beamer's local llama.cpp model server for K2-Horizon extraction
rem (Windows-on-ARM64, CPU-only). Beamer does NOT spawn this directly; it
rem triggers the Scheduled Task that runs this script once (src/model_setup.rs,
rem after a verified model download) and the task's own AtLogOn trigger
rem covers every login after that. Beamer must degrade gracefully when this
rem is absent or the model file is missing: the note is still captured, it
rem just is not extracted.
rem
rem %~dp0 resolves to this script's own directory (the fixed runtime path,
rem %LOCALAPPDATA%\Beamer\llama-k2horizon\ once installed — see hooks.nsh) so
rem this file is portable and carries no machine-specific path. The model
rem lives at a DIFFERENT fixed path (%USERPROFILE%\models\beamer\), resolved
rem explicitly below rather than relative to %~dp0.
cd /d "%~dp0"
llama-server.exe --models-dir "%USERPROFILE%\models\beamer" --models-preset "%~dp0llama-models-bearcave.ini" --models-max 1 --host 127.0.0.1 --port 8080
