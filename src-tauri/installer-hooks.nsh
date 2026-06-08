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

; On a real uninstall, offer to also wipe the launcher's stored data. Tauri
; only removes the install dir, leaving the config/token/cache behind under
; AppData, so a reinstall silently inherits old state. We delete both the
; ProjectDirs tree (defrag\launcher) and the identifier-keyed Tauri/plugin tree
; (racing.defrag.launcher), in Roaming and Local.
;
; Guarded by IfSilent so the updater's silent uninstall NEVER wipes user data -
; only an interactive uninstall, and only if the user confirms.
!macro NSIS_HOOK_POSTUNINSTALL
  IfSilent drl_keepdata
  MessageBox MB_YESNO|MB_ICONQUESTION "Also delete Defrag Racing Launcher settings, your saved token and cached data?$\n$\nYour demo files on this PC and your demos already backed up to defrag.racing are NOT affected." IDNO drl_keepdata
    ; App config / history / upload cache (directories::ProjectDirs)
    RMDir /r "$APPDATA\defrag\launcher"
    RMDir /r "$LOCALAPPDATA\defrag\launcher"
    RMDir "$APPDATA\defrag"
    RMDir "$LOCALAPPDATA\defrag"
    ; Tauri app + plugin data (store, window-state, webview) keyed by identifier
    RMDir /r "$APPDATA\racing.defrag.launcher"
    RMDir /r "$LOCALAPPDATA\racing.defrag.launcher"
  drl_keepdata:
!macroend
