; Beamer K2-Horizon local-extraction cleanup. `!include`d once by
; [bundle.windows.nsis].installer_hooks (see Dioxus.toml) at global script
; scope, between the generated install Section's SectionEnd and the
; generated "Section \"Uninstall\"". Defines only an "un."-prefixed section,
; which NSIS compiles into the uninstaller.
;
; Install-time placement is gone on purpose. The runtime (llama-server.exe +
; DLLs), the preset ini, the launchers, the Scheduled Task and the model are
; all components the app itself reconciles on launch (src/components/), so
; an install gets them however the exe arrived: this installer, a
; self-update, or a hand copy. This file used to embed the runtime from a
; hardcoded dev-machine path, so only installers built on that PC carried it;
; CI-built ARM installers now work the same as local ones.
;
; What remains is removing what the app put on disk. The paths and task
; name below must match src/components/ (Root::LocalData = %LOCALAPPDATA%\
; Beamer, Root::Models = %USERPROFILE%\models\beamer, llama::TASK_NAME).

!define K2H_RUNTIME_DIR "$LOCALAPPDATA\Beamer\llama-k2horizon"
!define K2H_TASK_NAME "Beamer K2-Horizon Server"

Section "un.K2HorizonCleanup"
  nsExec::ExecToLog 'schtasks /end /tn "${K2H_TASK_NAME}"'
  Pop $0
  nsExec::ExecToLog 'schtasks /delete /tn "${K2H_TASK_NAME}" /f'
  Pop $0
  nsExec::ExecToLog "taskkill /F /IM llama-server.exe /T"
  Pop $0

  RMDir /r "${K2H_RUNTIME_DIR}"
  ; The reconciler's swap siblings (src/components/archive.rs), present only
  ; if a runtime update was interrupted.
  RMDir /r "${K2H_RUNTIME_DIR}.old"
  RMDir /r "${K2H_RUNTIME_DIR}.staging"
  Delete "${K2H_RUNTIME_DIR}.zip.part"
  ; The model itself, not the whole models\beamer\ directory — other
  ; GGUFs may also live there. Decision 11 ("uninstall removes everything")
  ; is scoped to what this feature itself put on disk.
  Delete "$PROFILE\models\beamer\K2-Horizon-0.9B-Q8_0.gguf"
  Delete "$PROFILE\models\beamer\K2-Horizon-0.9B-Q8_0.gguf.part"
SectionEnd
