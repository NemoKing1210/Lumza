; Lumza uninstall hook (Tauri NSIS "installerHooks").
; The app registers itself for autostart by writing HKCU\...\Run\Lumza, which the installer does not
; know about, so it must be removed here or a ghost entry stays behind.
!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lumza"
  ; The "Delete the application data" checkbox of the uninstaller: settings, cache and logs.
  ${If} $DeleteAppDataCheckboxState == 1
    RMDir /r "$APPDATA\Lumza"
    RMDir /r "$LOCALAPPDATA\Lumza\logs"
    Delete "$LOCALAPPDATA\Lumza\cache.json"
  ${EndIf}
!macroend

; The installer records the install folder under HKCU\Software\<manufacturer>\<product> and the stock
; uninstaller leaves that key (and its empty parent) behind.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\${MANUFACTURER}\${PRODUCTNAME}"
  DeleteRegKey /ifempty HKCU "Software\${MANUFACTURER}"
!macroend
