' Launches start-llama-k2horizon.cmd with no visible window at all (0 =
' hidden, False = don't wait). Task Scheduler running a .cmd directly still
' flashes a console; routing through wscript.exe and WScript.Shell.Run is the
' standard way to suppress it, including the child llama-server.exe's own
' console (confirmed working this way on this machine).
'
' WScript.ScriptFullName resolves to THIS script's own path, so the .cmd it
' launches is found relative to wherever this file actually is on disk (the
' fixed runtime path, %LOCALAPPDATA%\Beamer\llama-k2horizon\ once installed —
' see hooks.nsh) rather than a hardcoded machine-specific path.
Set fso = CreateObject("Scripting.FileSystemObject")
scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
cmdPath = fso.BuildPath(scriptDir, "start-llama-k2horizon.cmd")

Set objShell = CreateObject("WScript.Shell")
objShell.Run """" & cmdPath & """", 0, False
