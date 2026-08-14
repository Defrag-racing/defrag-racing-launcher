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

  ; Take the right-click entry and our file type back out. Leaving them behind
  ; would put a menu item and an icon on every demo pointing at an executable
  ; that is no longer there.
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.dm_68\shell\PlayInDefragLauncher"
  DeleteRegKey HKCU "Software\Classes\DefragRacingLauncher.Demo"

  ; Only give the file type back if we are the one holding it. A default the
  ; user picked for somebody else is not ours to clear.
  ReadRegStr $0 HKCU "Software\Classes\.dm_68" ""
  StrCmp $0 "DefragRacingLauncher.Demo" 0 drl_keepassoc
    DeleteRegValue HKCU "Software\Classes\.dm_68" ""
  drl_keepassoc:
!macroend

; Put "Play in Defrag Launcher" into the right-click menu for .dm_68 files.
;
; Hung off the file EXTENSION, not off a program, so it sits next to whatever
; the user already opens demos with. It changes NO default: DemoCleaner3 keeps
; the file type, keeps its icon and keeps opening on a double-click. Becoming
; the default is offered once inside the app and is the user's call - see
; src/file_assoc.rs.
;
; Written here as well as at every app start so it works before the launcher
; has ever been run. HKCU because the installer is per-user (installMode:
; currentUser) - a machine-wide write would fail on a standard account.
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.dm_68\shell\PlayInDefragLauncher" "" "Play in Defrag Launcher"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.dm_68\shell\PlayInDefragLauncher" "Icon" '"$INSTDIR\${MAINBINARYNAME}.exe",0'
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.dm_68\shell\PlayInDefragLauncher\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
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
