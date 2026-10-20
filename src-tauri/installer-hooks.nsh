; Eclink NSIS installer hooks.
;
; Purpose: Eclink is the renamed Eclyzer Bridge. Tauri's NSIS template derives
; the uninstall registry key from ${PRODUCTNAME}:
;
;     !define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
;
; so changing the product name from "Eclyzer Bridge" to "Eclink" means the new
; installer writes a *different* uninstall key. Without cleanup, an upgraded
; machine ends up with two entries in Add/Remove Programs for one application,
; and the stale one uninstalls nothing useful.
;
; What this hook deliberately does NOT do:
;
;   * It never deletes user data. Application data lives in
;     %LOCALAPPDATA%\EclyzerBridge (no space) and is not referenced here at all.
;     That directory name is retained on purpose; see
;     docs/legacy-compatibility.md. Only the old *install* directory
;     (%LOCALAPPDATA%\Eclyzer Bridge, with a space) is touched, and only files
;     this project wrote are deleted.
;   * It never removes the legacy entry while the old executable is still there.
;     If a copy is running, the file is locked, the delete fails, and the entry
;     is kept so it still points at something real. A stale entry is a cosmetic
;     problem; an orphaned uninstallable program is not.
;   * It never touches the updater, the data directory, or any agent config.
;
; The sequence is: delete the known legacy binaries, then verify. Only if the
; executable is confirmed gone does it remove shortcuts and registry keys.

!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Eclyzer Bridge" "InstallLocation"
  StrCmp $R0 "" eclink_legacy_no_location

  ; Strip the surrounding quotes Tauri writes into InstallLocation.
  StrCpy $R1 $R0 1
  StrCmp $R1 '"' 0 +2
    StrCpy $R0 $R0 -1 1
  StrCpy $R1 $R0 1 -1
  StrCmp $R1 '"' 0 +2
    StrCpy $R0 $R0 -1

  ; Delete only the files the previous version installed. Anything else the user
  ; put in that directory is left alone.
  Delete "$R0\eclyzer-bridge.exe"
  Delete "$R0\eclyzer-bridge-helper.exe"
  Delete "$R0\uninstall.exe"
  RMDir "$R0"

  ; The decisive check. A running copy cannot be deleted, so this fails closed.
  IfFileExists "$R0\eclyzer-bridge.exe" eclink_legacy_keep

  ; The old binaries are gone, so the shortcuts and registry bookkeeping now
  ; point at nothing.
  Delete "$SMPROGRAMS\Eclyzer Bridge.lnk"
  RMDir "$SMPROGRAMS\Eclyzer Bridge"
  Delete "$DESKTOP\Eclyzer Bridge.lnk"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Eclyzer Bridge"
  DeleteRegKey HKCU "Software\Eclyzer Bridge contributors\Eclyzer Bridge"
  DeleteRegKey /ifempty HKCU "Software\Eclyzer Bridge contributors"
  DetailPrint "Eclink: removed the previous Eclyzer Bridge installation entry."
  Goto eclink_legacy_done

  eclink_legacy_keep:
    DetailPrint "Eclink: the previous Eclyzer Bridge executable is still in use, so its uninstall entry was kept."
    Goto eclink_legacy_done

  eclink_legacy_no_location:
    ; The key exists but records no install path, so it cannot launch anything.
    ; Confirm the key is actually there before deleting, so a fresh install does
    ; not create and then remove an empty key.
    ReadRegStr $R1 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Eclyzer Bridge" "DisplayName"
    StrCmp $R1 "" eclink_legacy_done
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Eclyzer Bridge"
    DetailPrint "Eclink: removed an empty Eclyzer Bridge uninstall entry."

  eclink_legacy_done:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Uninstalling Eclink removes application binaries and shortcuts. User data
  ; under %LOCALAPPDATA%\EclyzerBridge is intentionally preserved, matching the
  ; retention policy the product documented before the rename.
!macroend
