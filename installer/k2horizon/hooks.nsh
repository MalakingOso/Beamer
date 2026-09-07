; Beamer K2-Horizon local-extraction setup. `!include`d once by
; [bundle.windows.nsis].installer_hooks (see Dioxus.toml) at global script
; scope, between the generated install Section's SectionEnd and the
; generated "Section \"Uninstall\"". Defines its own hidden install-time
; section (leading "-" hides it from the components page and marks it
; non-optional) AND its own "un."-prefixed uninstall section — NSIS
; compiles both into their respective installer/uninstaller regardless of
; where in the script they're declared, so one file covers both directions.
;
; Runtime files (llama-server.exe + DLLs + the models-preset ini) and the
; downloaded model live at a FIXED per-user path, independent of whether
; this install is per-user or per-machine (install_mode = "Both") — Beamer
; runs asInvoker (never elevated, deliberate — see
; agent_docs/running_on_bearcave.md), and a per-machine install puts the app
; under Program Files, which an unelevated running beamer.exe can't write
; into later. src/model_setup.rs downloads the model itself; this hook only
; places the small (~24MB) runtime and registers the task DORMANT — starting
; the router before the model file exists leaves it stuck reporting
; "loading" forever with no error (confirmed empirically), so this file
; must never pass /run to schtasks.

!include "LogicLib.nsh"

!define K2H_RUNTIME_DIR "$LOCALAPPDATA\Beamer\llama-k2horizon"
!define K2H_TASK_NAME "Beamer K2-Horizon Server"
!define K2H_TASK_XML "${K2H_RUNTIME_DIR}\task.xml"
; Absolute, dev-machine path on purpose (see decision 1, "local build only" —
; this installer is only ever produced by `dx bundle` run locally on this
; checkout). NOT a runtime path — this only tells the NSIS *compiler* where
; to embed the files FROM; nothing here ever executes on an end-user
; machine. `[bundle.windows].resources` was tried first and does NOT
; actually stage arbitrary glob resources in this dioxus-cli version
; (confirmed empirically: only manganis `asset!()` output appears in the
; generated .nsi; a `resources` glob entry silently does not, no warning
; even), so this embeds the files directly via NSIS's own `File`
; instruction instead.
!define K2H_VENDOR_SRC "C:\Users\berkl\Programming\Beamer\vendor\llama-k2horizon"

Section "-K2HorizonSetup"
  DetailPrint "K2-Horizon setup starting"

  ; Stop anything already running from a previous manual setup or an earlier
  ; install of this feature, so the files below aren't locked and the new
  ; task definition doesn't fight an old process for port 8080. Harmless,
  ; expected-to-sometimes-fail no-op when nothing is running.
  nsExec::ExecToLog "taskkill /F /IM llama-server.exe /T"
  Pop $0

  SetOutPath "${K2H_RUNTIME_DIR}"
  ; /nonfatal: on CI (windows-latest, x86_64) vendor/llama-k2horizon/ never
  ; exists at build time — this must be a silent no-op there, not a compile
  ; failure, since CI still needs to produce its (K2-Horizon-less) x86_64
  ; installer. Verified empirically both ways (populated and empty source).
  File /nonfatal "${K2H_VENDOR_SRC}\*.*"
  SetOutPath $INSTDIR

  ; Task definition is written as XML rather than passed via `schtasks /tr
  ; "wscript.exe \"...\""` — that needs a nested-quote command-line string,
  ; and NSIS single-quoted literals do NOT interpret `\"` as an escaped
  ; quote (it passes the literal backslash through), which silently produced
  ; a malformed /tr value: schtasks received it without error surfacing
  ; anywhere (the exit code was never checked), so the task was never
  ; actually updated. XML splits Command and Arguments into separate
  ; elements, so the vbs path needs no quoting/escaping at all.
  ; schtasks /xml rejects a non-UTF-16 file with "unable to switch the
  ; encoding" even when the XML itself is well-formed ASCII (verified
  ; empirically: plain FileWrite/UTF-8 was refused outright) — it requires
  ; UTF-16 with a BOM, hence FileWriteUTF16LE /BOM on the first line only.
  FileOpen $1 "${K2H_TASK_XML}" w
  FileWriteUTF16LE /BOM $1 '<?xml version="1.0" encoding="UTF-16"?>$\r$\n'
  FileWriteUTF16LE $1 '<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">$\r$\n'
  FileWriteUTF16LE $1 '  <Triggers>$\r$\n'
  FileWriteUTF16LE $1 '    <LogonTrigger>$\r$\n'
  FileWriteUTF16LE $1 '      <Enabled>true</Enabled>$\r$\n'
  FileWriteUTF16LE $1 '    </LogonTrigger>$\r$\n'
  FileWriteUTF16LE $1 '  </Triggers>$\r$\n'
  FileWriteUTF16LE $1 '  <Principals>$\r$\n'
  FileWriteUTF16LE $1 '    <Principal id="Author">$\r$\n'
  FileWriteUTF16LE $1 '      <LogonType>InteractiveToken</LogonType>$\r$\n'
  FileWriteUTF16LE $1 '      <RunLevel>LeastPrivilege</RunLevel>$\r$\n'
  FileWriteUTF16LE $1 '    </Principal>$\r$\n'
  FileWriteUTF16LE $1 '  </Principals>$\r$\n'
  FileWriteUTF16LE $1 '  <Settings>$\r$\n'
  FileWriteUTF16LE $1 '    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>$\r$\n'
  FileWriteUTF16LE $1 '    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>$\r$\n'
  FileWriteUTF16LE $1 '    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>$\r$\n'
  FileWriteUTF16LE $1 '    <StartWhenAvailable>true</StartWhenAvailable>$\r$\n'
  FileWriteUTF16LE $1 '  </Settings>$\r$\n'
  FileWriteUTF16LE $1 '  <Actions Context="Author">$\r$\n'
  FileWriteUTF16LE $1 '    <Exec>$\r$\n'
  FileWriteUTF16LE $1 '      <Command>wscript.exe</Command>$\r$\n'
  FileWriteUTF16LE $1 '      <Arguments>"${K2H_RUNTIME_DIR}\start-llama-k2horizon-hidden.vbs"</Arguments>$\r$\n'
  FileWriteUTF16LE $1 '    </Exec>$\r$\n'
  FileWriteUTF16LE $1 '  </Actions>$\r$\n'
  FileWriteUTF16LE $1 '</Task>$\r$\n'
  FileClose $1

  ; /f forces overwrite of an existing task definition of the same name, so
  ; running this installer again over an already-set-up machine UPDATES the
  ; task rather than erroring or duplicating it. No /run: registered
  ; dormant, per the header comment above.
  nsExec::ExecToLog 'schtasks /create /tn "${K2H_TASK_NAME}" /xml "${K2H_TASK_XML}" /f'
  Pop $0
  ${If} $0 != 0
    DetailPrint "K2-Horizon task registration failed (schtasks exit code $0)"
    MessageBox MB_OK|MB_ICONEXCLAMATION "K2-Horizon Scheduled Task registration failed (exit code $0). Local extraction will not start automatically — see agent_docs/running_on_bearcave.md for manual setup."
  ${EndIf}
SectionEnd

Section "un.K2HorizonCleanup"
  nsExec::ExecToLog 'schtasks /end /tn "${K2H_TASK_NAME}"'
  Pop $0
  nsExec::ExecToLog 'schtasks /delete /tn "${K2H_TASK_NAME}" /f'
  Pop $0
  nsExec::ExecToLog "taskkill /F /IM llama-server.exe /T"
  Pop $0

  RMDir /r "${K2H_RUNTIME_DIR}"
  ; The model itself, not the whole models\beamer\ directory — Gemma/S1-mini
  ; GGUFs may also live there. Decision 11 ("uninstall removes everything")
  ; is scoped to what this feature itself put on disk.
  Delete "$PROFILE\models\beamer\K2-Horizon-0.9B-Q8_0.gguf"
  Delete "$PROFILE\models\beamer\K2-Horizon-0.9B-Q8_0.gguf.part"
SectionEnd
