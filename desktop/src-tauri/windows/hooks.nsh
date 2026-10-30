; Stackline was called swarmdash before. The setup program of a renamed app
; does not find the old one, so it would stay installed next to Stackline.
; After installing, remove swarmdash quietly. Its uninstaller keeps the
; settings and logs, which Stackline has moved to its own folder.
!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $R0 SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\swarmdash" "UninstallString"
  ReadRegStr $R1 SHCTX "Software\${MANUFACTURER}\swarmdash" ""
  ${If} $R0 != ""
  ${AndIf} $R1 != ""
  ${AndIf} $R1 != $INSTDIR
    ExecWait '$R0 /S _?=$R1'
    Delete "$R1\uninstall.exe"
    RMDir "$R1"
    DeleteRegKey SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\swarmdash"
    DeleteRegKey SHCTX "Software\${MANUFACTURER}\swarmdash"
  ${EndIf}
!macroend
