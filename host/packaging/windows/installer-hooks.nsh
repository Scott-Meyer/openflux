; NSIS installer hooks for OpenFlux Studio, included by the Tauri NSIS
; template through bundle.windows.nsis.installerHooks.
;
; openfluxd starts at sign-in through the per-user Run key. Sidecars are
; installed next to the main executable in $INSTDIR, so the Run value points
; at $INSTDIR\openfluxd.exe and follows the install location.
;
; Before files are replaced or removed, the installed daemon is asked to stop
; with `openfluxd --request-stop`. The daemon is never killed. Its answer
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

!define OPENFLUX_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define OPENFLUX_RUN_VALUE "OpenFlux"
!define OPENFLUX_STOP_TIMEOUT_SECONDS 900

!macro OPENFLUX_DEFINE_STOP_DAEMON PREFIX
Function ${PREFIX}OpenFluxStopDaemon
  Push $0 ; exit status of --request-stop
  Push $1 ; seconds waited
  Push $2 ; its output
  StrCpy $1 0
  IfFileExists "$INSTDIR\openfluxd.exe" openflux_ask openflux_done
  openflux_ask:
    nsExec::ExecToStack '"$INSTDIR\openfluxd.exe" --request-stop'
    Pop $0
    Pop $2
    StrCmp $0 "0" openflux_done
    StrCmp $0 "4" openflux_done
    StrCmp $0 "3" openflux_deferred openflux_failed
  openflux_deferred:
    IntCmp $1 0 0 openflux_wait openflux_wait
      DetailPrint "OpenFlux is finishing work on the keyboard. Waiting for it to complete."
    openflux_wait:
    IntCmp $1 ${OPENFLUX_STOP_TIMEOUT_SECONDS} openflux_timeout 0 openflux_timeout
    Sleep 5000
    IntOp $1 $1 + 5
    Goto openflux_ask
  openflux_timeout:
    MessageBox MB_OK|MB_ICONEXCLAMATION "OpenFlux is still working on the keyboard. Nothing was changed. Leave the keyboard connected, and run this again when it has finished." /SD IDOK
    Goto openflux_abort
  openflux_failed:
    DetailPrint "openfluxd --request-stop returned $0: $2"
    MessageBox MB_OK|MB_ICONSTOP "OpenFlux could not confirm that its background service has stopped (openfluxd --request-stop returned $0). Nothing was changed. Quit OpenFlux from its tray icon, then run this again.$\r$\n$\r$\n$2" /SD IDOK
  openflux_abort:
    Pop $2
    Pop $1
    Pop $0
    Abort
  openflux_done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
!macroend

!insertmacro OPENFLUX_DEFINE_STOP_DAEMON ""
!insertmacro OPENFLUX_DEFINE_STOP_DAEMON "un."

!macro NSIS_HOOK_PREINSTALL
  Call OpenFluxStopDaemon
!macroend

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "${OPENFLUX_RUN_KEY}" "${OPENFLUX_RUN_VALUE}" '"$INSTDIR\openfluxd.exe"'
  ; Started as the signed-in user, never with the installer's elevation.
  nsis_tauri_utils::RunAsUser "$INSTDIR\openfluxd.exe" ""
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Call un.OpenFluxStopDaemon
  ; An update runs the previous uninstaller with /UPDATE; the new installer
  ; writes the Run value again, so it is only removed on a real uninstall.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "${OPENFLUX_RUN_KEY}" "${OPENFLUX_RUN_VALUE}"
  ${EndIf}
!macroend
