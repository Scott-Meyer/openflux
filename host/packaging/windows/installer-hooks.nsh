; NSIS installer hooks for Keyglass Studio, included by the Tauri NSIS
; template through bundle.windows.nsis.installerHooks.
;
; keyglassd starts at sign-in through the per-user Run key. Sidecars are
; installed next to the main executable in $INSTDIR, so the Run value points
; at $INSTDIR\keyglassd.exe and follows the install location.
;
; Before files are replaced or removed, the installed daemon is asked to stop
; with `keyglassd --request-stop`. The daemon is never killed. Its answer
; decides what happens:
;   0  it was idle and has exited: continue
;   4  it is not running: continue
;   3  it is flashing, converting or restoring the keyboard and exits by
;      itself once that is done: wait for it, up to 15 minutes, then stop
;      the installer with an explanation
;   anything else (including a daemon that can't be run): stop the installer
;      with an explanation; nothing has been changed at that point
; When Studio installs an update it has already stopped the daemon, so the
; hooks see 4.

!define KEYGLASS_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define KEYGLASS_RUN_VALUE "Keyglass"
!define KEYGLASS_STOP_TIMEOUT_SECONDS 900

!macro KEYGLASS_DEFINE_STOP_DAEMON PREFIX
Function ${PREFIX}KeyglassStopDaemon
  Push $0 ; exit status of --request-stop
  Push $1 ; seconds waited
  Push $2 ; its output
  StrCpy $1 0
  IfFileExists "$INSTDIR\keyglassd.exe" keyglass_ask keyglass_done
  keyglass_ask:
    nsExec::ExecToStack '"$INSTDIR\keyglassd.exe" --request-stop'
    Pop $0
    Pop $2
    StrCmp $0 "0" keyglass_done
    StrCmp $0 "4" keyglass_done
    StrCmp $0 "3" keyglass_deferred keyglass_failed
  keyglass_deferred:
    IntCmp $1 0 0 keyglass_wait keyglass_wait
      DetailPrint "Keyglass is finishing work on the keyboard. Waiting for it to complete."
    keyglass_wait:
    IntCmp $1 ${KEYGLASS_STOP_TIMEOUT_SECONDS} keyglass_timeout 0 keyglass_timeout
    Sleep 5000
    IntOp $1 $1 + 5
    Goto keyglass_ask
  keyglass_timeout:
    MessageBox MB_OK|MB_ICONEXCLAMATION "Keyglass is still working on the keyboard. Nothing was changed. Leave the keyboard connected, and run this again when it has finished." /SD IDOK
    Goto keyglass_abort
  keyglass_failed:
    DetailPrint "keyglassd --request-stop returned $0: $2"
    MessageBox MB_OK|MB_ICONSTOP "Keyglass could not confirm that its background service has stopped (keyglassd --request-stop returned $0). Nothing was changed. Quit Keyglass from its tray icon, then run this again.$\r$\n$\r$\n$2" /SD IDOK
  keyglass_abort:
    Pop $2
    Pop $1
    Pop $0
    Abort
  keyglass_done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
!macroend

!insertmacro KEYGLASS_DEFINE_STOP_DAEMON ""
!insertmacro KEYGLASS_DEFINE_STOP_DAEMON "un."

!macro NSIS_HOOK_PREINSTALL
  Call KeyglassStopDaemon
!macroend

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "${KEYGLASS_RUN_KEY}" "${KEYGLASS_RUN_VALUE}" '"$INSTDIR\keyglassd.exe"'
  ; Started as the signed-in user, never with the installer's elevation.
  nsis_tauri_utils::RunAsUser "$INSTDIR\keyglassd.exe" ""
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Call un.KeyglassStopDaemon
  ; An update runs the previous uninstaller with /UPDATE; the new installer
  ; writes the Run value again, so it is only removed on a real uninstall.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "${KEYGLASS_RUN_KEY}" "${KEYGLASS_RUN_VALUE}"
  ${EndIf}
!macroend
