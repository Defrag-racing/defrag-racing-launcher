; Custom NSIS installer hooks for the Defrag Racing Launcher.
;
; Tauri's generated installer already stops the launcher process before
; (un)installing, but the embedded demo-player engine (oDFe.x64.exe) runs as a
; SEPARATE process, outside the launcher. If it's still running it keeps
; resources\odfe\oDFe.x64.exe locked, so the install fails with
; "Error opening file for writing: ...\resources\odfe\oDFe.x64.exe".
;
; Kill it (and any children) up front so upgrades are seamless. Silent and
; best-effort: taskkill just returns nonzero when the process isn't running.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /T /IM oDFe.x64.exe'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /T /IM oDFe.x64.exe'
  Pop $0
!macroend
