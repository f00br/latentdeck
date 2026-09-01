Unicode true
RequestExecutionLevel user
ManifestDPIAware true
ManifestSupportedOS all
CRCCheck force
SetCompressor /SOLID lzma
SetDatablockOptimize on
SetDateSave off
ShowInstDetails show
ShowUninstDetails show

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinCore.nsh"

!ifndef OUTPUT_PATH
  !error "OUTPUT_PATH is required"
!endif
!ifndef PACK_VERSION
  !error "PACK_VERSION is required"
!endif
!ifndef PRODUCT_VERSION4
  !error "PRODUCT_VERSION4 is required"
!endif
!ifndef ARCHIVE_NAME
  !error "ARCHIVE_NAME is required"
!endif
!ifndef ARCHIVE_SHA256
  !error "ARCHIVE_SHA256 is required"
!endif
!ifndef ARCHIVE_LENGTH
  !error "ARCHIVE_LENGTH is required"
!endif
!ifndef ESTIMATED_SIZE_KIB
  !error "ESTIMATED_SIZE_KIB is required"
!endif
!ifndef HELPER_PATH
  !error "HELPER_PATH is required"
!endif
!ifndef INSTALL_METADATA_PATH
  !error "INSTALL_METADATA_PATH is required"
!endif
!ifndef LICENSE_PATH
  !error "LICENSE_PATH is required"
!endif
!ifndef NOTICES_PATH
  !error "NOTICES_PATH is required"
!endif
!ifndef NSIS_COPYING_PATH
  !error "NSIS_COPYING_PATH is required"
!endif
!ifndef INSTALLER_SBOM_PATH
  !error "INSTALLER_SBOM_PATH is required"
!endif
!ifndef RUST_LICENSES_PATH
  !error "RUST_LICENSES_PATH is required"
!endif
!ifndef ICON_PATH
  !error "ICON_PATH is required"
!endif

!define PRODUCT_NAME "LatentDeck H3 Codec Pack"
!define PACK_ID "org.latentdeck.h3"
!define HELPER_FILE "latentdeck-codec-pack-installer.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PACK_ID}.${PACK_VERSION}"
!define PACK_DESTINATION "$LOCALAPPDATA\LatentDeck\CodecPacks\${PACK_ID}\${PACK_VERSION}"
!define MAINTENANCE_PARENT "$LOCALAPPDATA\LatentDeck\CodecPackMaintenance\${PACK_ID}"
!define MAINTENANCE_DESTINATION "${MAINTENANCE_PARENT}\${PACK_VERSION}"
!define MAINTENANCE_STAGE "${MAINTENANCE_PARENT}\.install-${PACK_VERSION}"
!define MAINTENANCE_BACKUP "${MAINTENANCE_PARENT}\.backup-${PACK_VERSION}"
!define LIFECYCLE_MUTEX "Global\LatentDeck.CodecPackLifecycle.org.latentdeck.h3"
!define WIN32_FIND_DATA_STRUCT "(i, l, l, l, i, i, i, i, &w260, &w14) p"

!macro ProbeSafeDirectory PATH PRESENT_VAR FAILURE_LABEL
  StrCpy ${PRESENT_VAR} "0"
  System::Call 'kernel32::GetFileAttributesW(w "${PATH}") i.r8 ?e'
  Pop $9
  ${If} $8 == -1
    ${If} $9 != 2
    ${AndIf} $9 != 3
      Goto ${FAILURE_LABEL}
    ${EndIf}
  ${Else}
    IntOp $9 $8 & 0x400
    ${If} $9 != 0
      Goto ${FAILURE_LABEL}
    ${EndIf}
    IntOp $9 $8 & 0x10
    ${If} $9 == 0
      Goto ${FAILURE_LABEL}
    ${EndIf}
    StrCpy ${PRESENT_VAR} "1"
  ${EndIf}
!macroend

; Maintenance trees are deliberately flat. Never recurse through a user-writable
; path: an unexpected file or directory makes the transaction fail closed, and
; a reparse-point directory is rejected before any child is touched.
!macro RemoveKnownMaintenanceTree PATH FAILURE_LABEL
  System::Call 'kernel32::GetFileAttributesW(w "${PATH}") i.r8 ?e'
  Pop $9
  ${If} $8 != -1
    IntOp $9 $8 & 0x400
    ${If} $9 != 0
      Goto ${FAILURE_LABEL}
    ${EndIf}
    Delete "${PATH}\${HELPER_FILE}"
    Delete "${PATH}\install-metadata.json"
    Delete "${PATH}\THIRD_PARTY_NOTICES.md"
    Delete "${PATH}\INSTALLER_NSIS_COPYING.txt"
    Delete "${PATH}\INSTALLER_RUST_LICENSES.txt"
    Delete "${PATH}\installer-SBOM.cdx.json"
    Delete "${PATH}\Uninstall.exe"
    ClearErrors
    RMDir "${PATH}"
    IfErrors ${FAILURE_LABEL}
  ${Else}
    ${If} $9 != 2
    ${AndIf} $9 != 3
      Goto ${FAILURE_LABEL}
    ${EndIf}
  ${EndIf}
!macroend

; A separately authorized signed build supplies one external command containing
; `%1`. NSIS invokes it for the generated uninstaller and then for setup.
!ifdef SIGNING_COMMAND
  !uninstfinalize '${SIGNING_COMMAND}' = 0
  !finalize '${SIGNING_COMMAND}' = 0
!endif

Name "${PRODUCT_NAME} ${PACK_VERSION}"
OutFile "${OUTPUT_PATH}"
InstallDir "${MAINTENANCE_DESTINATION}"
BrandingText "LatentDeck 0.1"
Icon "${ICON_PATH}"
UninstallIcon "${ICON_PATH}"

VIProductVersion "${PRODUCT_VERSION4}"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PACK_VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "LatentDeck H3 Codec Pack Setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${PACK_VERSION}"
VIAddVersionKey /LANG=1033 "CompanyName" "LatentDeck Project"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Apache-2.0; see bundled notices"

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_PATH}"
!define MUI_UNICON "${ICON_PATH}"
!define MUI_FINISHPAGE_TITLE "H3 Codec Pack ${PACK_VERSION} installed"
!define MUI_FINISHPAGE_TEXT "Restart LatentDeck and LatentPlayer, then confirm Codec Manager selected H3 Codec Pack ${PACK_VERSION}. The TAEH3 decoder weight remains a separate explicit selection."
!define MUI_UNFINISHPAGE_TITLE "H3 Codec Pack ${PACK_VERSION} removed"
!define MUI_UNFINISHPAGE_TEXT "Only H3 Codec Pack ${PACK_VERSION} was removed. LatentDeck, LatentPlayer, cartridges, and decoder selection were not changed."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${LICENSE_PATH}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

Var InstallResult
Var InstallOutput
Var FreshInstall
Var TransactionMutex
Var HadMaintenance
Var ProgramDataRoot
Var MaintenanceRegistryRemoved
Var MaintenanceRegistryTouched

Function ResolveProgramDataRoot
  System::Call 'shell32::SHGetFolderPathW(p 0, i ${CSIDL_COMMON_APPDATA}, p 0, i 0, t.r0) i.r1'
  ${If} $1 != 0
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Windows could not resolve the shared Codec Pack discovery root."
    SetErrorLevel 40
    Abort
  ${EndIf}
  StrCpy $ProgramDataRoot $0
FunctionEnd

Function un.ResolveProgramDataRoot
  System::Call 'shell32::SHGetFolderPathW(p 0, i ${CSIDL_COMMON_APPDATA}, p 0, i 0, t.r0) i.r1'
  ${If} $1 != 0
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Windows could not resolve the shared Codec Pack discovery root."
    SetErrorLevel 40
    Abort
  ${EndIf}
  StrCpy $ProgramDataRoot $0
FunctionEnd

Function AcquireTransactionMutex
  System::Call 'kernel32::CreateMutexW(p 0, i 1, w "${LIFECYCLE_MUTEX}") p.r0 ?e'
  Pop $1
  ${If} $0 == 0
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Windows could not create the Codec Pack lifecycle mutex."
    SetErrorLevel 40
    Abort
  ${EndIf}
  ${If} $1 == 183
    System::Call 'kernel32::CloseHandle(p r0)'
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Another H3 Codec Pack setup or uninstall operation is already active."
    SetErrorLevel 40
    Abort
  ${EndIf}
  StrCpy $TransactionMutex $0
FunctionEnd

Function un.AcquireTransactionMutex
  System::Call 'kernel32::CreateMutexW(p 0, i 1, w "${LIFECYCLE_MUTEX}") p.r0 ?e'
  Pop $1
  ${If} $0 == 0
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Windows could not create the Codec Pack lifecycle mutex."
    SetErrorLevel 40
    Abort
  ${EndIf}
  ${If} $1 == 183
    System::Call 'kernel32::CloseHandle(p r0)'
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Another H3 Codec Pack setup or uninstall operation is already active."
    SetErrorLevel 40
    Abort
  ${EndIf}
  StrCpy $TransactionMutex $0
FunctionEnd

Function NormalizeHelperExit
  StrCmp $InstallResult "0" helper_exit_known
  StrCmp $InstallResult "10" helper_exit_known
  StrCmp $InstallResult "20" helper_exit_known
  StrCmp $InstallResult "30" helper_exit_known
  StrCmp $InstallResult "31" helper_exit_known
  StrCmp $InstallResult "40" helper_exit_known
  StrCmp $InstallResult "50" helper_exit_known
  StrCpy $InstallResult "70"
helper_exit_known:
FunctionEnd

Function un.NormalizeHelperExit
  StrCmp $InstallResult "0" helper_exit_known
  StrCmp $InstallResult "10" helper_exit_known
  StrCmp $InstallResult "20" helper_exit_known
  StrCmp $InstallResult "30" helper_exit_known
  StrCmp $InstallResult "31" helper_exit_known
  StrCmp $InstallResult "40" helper_exit_known
  StrCmp $InstallResult "50" helper_exit_known
  StrCpy $InstallResult "70"
helper_exit_known:
FunctionEnd

Function .onInit
  SetShellVarContext current
  StrCpy $INSTDIR "${MAINTENANCE_DESTINATION}"
  Call ResolveProgramDataRoot
  Call AcquireTransactionMutex
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} requires 64-bit Windows."
    SetErrorLevel 10
    Abort
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
  StrCpy $INSTDIR "${MAINTENANCE_DESTINATION}"
  Call un.ResolveProgramDataRoot
  Call un.AcquireTransactionMutex
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} requires 64-bit Windows."
    SetErrorLevel 10
    Abort
  ${EndIf}
FunctionEnd

Section "Install H3 Codec Pack" SEC_MAIN
  SectionIn RO
  AddSize ${ESTIMATED_SIZE_KIB}
  StrCpy $FreshInstall "0"
  StrCpy $MaintenanceRegistryTouched "0"
  ; `/D=...` must never redirect maintenance bytes into the integrity-closed pack.
  StrCpy $INSTDIR "${MAINTENANCE_DESTINATION}"
  ; Reject every existing maintenance component below the known LocalAppData
  ; root before the helper or installer mutates anything.
  !insertmacro ProbeSafeDirectory "$LOCALAPPDATA\LatentDeck" $7 maintenance_root_unsafe
  !insertmacro ProbeSafeDirectory "$LOCALAPPDATA\LatentDeck\CodecPackMaintenance" $7 maintenance_root_unsafe
  !insertmacro ProbeSafeDirectory "${MAINTENANCE_PARENT}" $7 maintenance_root_unsafe

  IfFileExists "$EXEDIR\${ARCHIVE_NAME}" payload_present
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "Required payload is missing.$\r$\n$\r$\nPlace ${ARCHIVE_NAME} beside this setup.exe and run setup again."
    SetErrorLevel 20
    Abort

payload_present:
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=${HELPER_FILE}" "${HELPER_PATH}"

  DetailPrint "Validating ${ARCHIVE_NAME} and installing immutable pack ${PACK_VERSION}..."
  nsExec::ExecToStack '"$PLUGINSDIR\${HELPER_FILE}" --local-app-data "$LOCALAPPDATA" --program-data "$ProgramDataRoot" install --archive "$EXEDIR\${ARCHIVE_NAME}" --expected-sha256 "${ARCHIVE_SHA256}" --expected-length "${ARCHIVE_LENGTH}" --expected-version "${PACK_VERSION}"'
  Pop $InstallResult
  Pop $InstallOutput
  Call NormalizeHelperExit

  ${If} $InstallResult == "0"
    StrCpy $FreshInstall "1"
  ${ElseIf} $InstallResult == "30"
    DetailPrint "The immutable pack bytes are already installed; refreshing maintenance metadata only."
  ${Else}
    DetailPrint "$InstallOutput"
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "H3 Codec Pack installation failed (code $InstallResult).$\r$\n$\r$\n$InstallOutput"
    SetErrorLevel $InstallResult
    Abort
  ${EndIf}

  ; Recover or remove only deterministic residue from an interrupted transaction.
  !insertmacro ProbeSafeDirectory "${MAINTENANCE_BACKUP}" $HadMaintenance maintenance_failed
  ${If} $HadMaintenance == "0"
    Goto maintenance_stage_cleanup
  ${EndIf}
  !insertmacro ProbeSafeDirectory "$INSTDIR" $7 maintenance_failed
  ${If} $7 == "1"
    Goto maintenance_discard_old_backup
  ${EndIf}
  Goto maintenance_restore_backup

maintenance_discard_old_backup:
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_BACKUP}" maintenance_failed
  Goto maintenance_stage_cleanup

maintenance_restore_backup:
  ClearErrors
  Rename "${MAINTENANCE_BACKUP}" "$INSTDIR"
  IfErrors maintenance_failed

maintenance_stage_cleanup:
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_STAGE}" maintenance_failed
  ClearErrors
  CreateDirectory "${MAINTENANCE_STAGE}"
  SetOutPath "${MAINTENANCE_STAGE}"
  File "/oname=install-metadata.json" "${INSTALL_METADATA_PATH}"
  File "/oname=THIRD_PARTY_NOTICES.md" "${NOTICES_PATH}"
  File "/oname=INSTALLER_NSIS_COPYING.txt" "${NSIS_COPYING_PATH}"
  File "/oname=INSTALLER_RUST_LICENSES.txt" "${RUST_LICENSES_PATH}"
  File "/oname=installer-SBOM.cdx.json" "${INSTALLER_SBOM_PATH}"
  WriteUninstaller "${MAINTENANCE_STAGE}\Uninstall.exe"
  IfErrors maintenance_failed
  ; SetOutPath also changes the process current directory. Leave the stage
  ; before renaming or removing it; Windows will not move the current folder.
  SetOutPath "$PLUGINSDIR"

  ; Publish a complete maintenance tree with a same-volume rename.
  !insertmacro ProbeSafeDirectory "$INSTDIR" $HadMaintenance maintenance_failed
  ${If} $HadMaintenance == "1"
    Goto maintenance_move_old
  ${EndIf}
  Goto maintenance_publish_stage

maintenance_move_old:
  ClearErrors
  Rename "$INSTDIR" "${MAINTENANCE_BACKUP}"
  IfErrors maintenance_failed
  StrCpy $HadMaintenance "1"

maintenance_publish_stage:
  ClearErrors
  Rename "${MAINTENANCE_STAGE}" "$INSTDIR"
  IfErrors maintenance_publish_failed

  ClearErrors
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME} ${PACK_VERSION}"
  IfErrors maintenance_registry_failed
  StrCpy $MaintenanceRegistryTouched "1"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${PACK_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "LatentDeck Project"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "${PACK_DESTINATION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" ${ESTIMATED_SIZE_KIB}
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  IfErrors maintenance_registry_failed

  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_BACKUP}" maintenance_cleanup_failed
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_STAGE}" maintenance_cleanup_failed

  SetErrorLevel 0
  Goto install_done

maintenance_root_unsafe:
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The fixed Codec Pack maintenance path contains an inaccessible, non-directory, or reparse-point component. No installation changes were made."
  SetErrorLevel 40
  Abort

maintenance_cleanup_failed:
  DetailPrint "The pack and Installed Apps entry are complete, but stale maintenance residue could not be removed safely."
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "H3 Codec Pack ${PACK_VERSION} is installed, but stale maintenance files could not be removed safely. Do not delete them manually; close other maintenance processes and run setup again."
  SetErrorLevel 60
  Abort

maintenance_publish_failed:
  ${If} $HadMaintenance == "1"
    ClearErrors
    Rename "${MAINTENANCE_BACKUP}" "$INSTDIR"
    IfErrors maintenance_restore_failed
  ${EndIf}
  Goto maintenance_failed

maintenance_restore_failed:
  DetailPrint "Publishing the replacement maintenance tree failed, and the prior tree could not be restored from its exact backup."
  ${If} $FreshInstall == "1"
    Goto maintenance_failed
  ${EndIf}
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The immutable pack remains installed, but its prior Windows maintenance tree could not be restored. It remains at ${MAINTENANCE_BACKUP}. Do not move or delete it; close other maintenance processes and run setup again."
  SetErrorLevel 62
  Abort

maintenance_registry_failed:
  DetailPrint "The complete maintenance tree was published, but registry maintenance failed."
  Goto maintenance_failed

maintenance_failed:
  ; A stage extraction or WriteUninstaller failure may arrive here while the
  ; process current directory is still the transaction stage.
  SetOutPath "$PLUGINSDIR"
  DetailPrint "Failed to create Installed Apps maintenance data."
  ${If} $FreshInstall == "1"
    nsExec::ExecToStack '"$PLUGINSDIR\${HELPER_FILE}" --local-app-data "$LOCALAPPDATA" --program-data "$ProgramDataRoot" uninstall --version "${PACK_VERSION}"'
    Pop $InstallResult
    Pop $InstallOutput
    Call NormalizeHelperExit
    ${If} $InstallResult == "0"
      Goto maintenance_rollback_complete
    ${ElseIf} $InstallResult == "31"
      Goto maintenance_rollback_complete
    ${Else}
      DetailPrint "Pack rollback failed (code $InstallResult): $InstallOutput"
      IfSilent +2
        MessageBox MB_OK|MB_ICONSTOP "Windows maintenance registration failed, and the newly installed pack could not be rolled back. The exact pack may remain installed. Close LatentDeck, LatentPlayer, and Codec Pack workers, then run setup again."
      SetErrorLevel 61
      Abort
    ${EndIf}
  ${Else}
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "The existing immutable pack remains installed, but its Windows Installed Apps maintenance data could not be repaired. Run setup again after closing other maintenance processes."
    SetErrorLevel 60
    Abort
  ${EndIf}

maintenance_rollback_complete:
  ; Delete only a key this transaction actually created or replaced.
  ; DeleteRegKey reports an error when the key is already absent, which is the
  ; expected state when maintenance publication failed before registry writes.
  ${If} $MaintenanceRegistryTouched == "1"
    ClearErrors
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    IfErrors maintenance_rollback_cleanup_failed
  ${EndIf}
  !insertmacro RemoveKnownMaintenanceTree "$INSTDIR" maintenance_rollback_cleanup_failed
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_STAGE}" maintenance_rollback_cleanup_failed
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_BACKUP}" maintenance_rollback_cleanup_failed
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The Codec Pack could not be registered in Windows Installed Apps. No new pack version was retained."
  SetErrorLevel 60
  Abort

maintenance_rollback_cleanup_failed:
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The new Codec Pack was rolled back, but maintenance residue could not be removed safely. Close other maintenance processes and run setup again."
  SetErrorLevel 60
  Abort

install_done:
SectionEnd

Section "Uninstall"
  StrCpy $MaintenanceRegistryRemoved "0"
  !insertmacro ProbeSafeDirectory "$LOCALAPPDATA\LatentDeck" $7 uninstall_maintenance_root_unsafe
  !insertmacro ProbeSafeDirectory "$LOCALAPPDATA\LatentDeck\CodecPackMaintenance" $7 uninstall_maintenance_root_unsafe
  !insertmacro ProbeSafeDirectory "${MAINTENANCE_PARENT}" $7 uninstall_maintenance_root_unsafe
  !insertmacro ProbeSafeDirectory "$INSTDIR" $7 uninstall_maintenance_root_unsafe
  ${If} $7 != "1"
    Goto uninstall_maintenance_root_unsafe
  ${EndIf}
  ; Inventory the current flat maintenance tree before removing the pack. An
  ; unknown entry must not survive until after the Installed Apps key is gone.
  System::Call '*${WIN32_FIND_DATA_STRUCT} .r4'
  System::Call 'kernel32::FindFirstFileW(w "$INSTDIR\*", p r4) p.r8 ?e'
  Pop $5
  ${If} $8 == -1
    System::Free $4
    Goto uninstall_maintenance_root_unsafe
  ${EndIf}

uninstall_inventory_next:
  System::Call '*$4(i .r6, l ., l ., l ., i ., i ., i ., i ., &w260 .r9, &w14 .)'
  StrCmp $9 "." uninstall_inventory_advance
  StrCmp $9 ".." uninstall_inventory_advance
  StrCmp $9 "Uninstall.exe" uninstall_inventory_known
  StrCmp $9 "install-metadata.json" uninstall_inventory_known
  StrCmp $9 "THIRD_PARTY_NOTICES.md" uninstall_inventory_known
  StrCmp $9 "INSTALLER_NSIS_COPYING.txt" uninstall_inventory_known
  StrCmp $9 "INSTALLER_RUST_LICENSES.txt" uninstall_inventory_known
  StrCmp $9 "installer-SBOM.cdx.json" uninstall_inventory_known
  StrCmp $9 "${HELPER_FILE}" uninstall_inventory_known
  Goto uninstall_inventory_unsafe

uninstall_inventory_known:
  System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR\$9") i.r6 ?e'
  Pop $5
  ${If} $6 == -1
    Goto uninstall_inventory_unsafe
  ${EndIf}
  IntOp $5 $6 & 0x410
  ${If} $5 != 0
    Goto uninstall_inventory_unsafe
  ${EndIf}

uninstall_inventory_advance:
  System::Call 'kernel32::FindNextFileW(p r8, p r4) i.r6 ?e'
  Pop $5
  ${If} $6 == 0
    ${If} $5 == 18
      Goto uninstall_inventory_done
    ${EndIf}
    Goto uninstall_inventory_unsafe
  ${EndIf}
  Goto uninstall_inventory_next

uninstall_inventory_unsafe:
  System::Call 'kernel32::FindClose(p r8)'
  System::Free $4
  Goto uninstall_maintenance_root_unsafe

uninstall_inventory_done:
  System::Call 'kernel32::FindClose(p r8)'
  System::Free $4
  ; The generated uninstaller carries its own build-bound helper. Never execute a
  ; mutable helper from the user-writable maintenance directory.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  ClearErrors
  File "/oname=${HELPER_FILE}" "${HELPER_PATH}"
  IfErrors uninstall_helper_extract_failed

uninstall_retry:
  nsExec::ExecToStack '"$PLUGINSDIR\${HELPER_FILE}" --local-app-data "$LOCALAPPDATA" --program-data "$ProgramDataRoot" uninstall --version "${PACK_VERSION}"'
  Pop $InstallResult
  Pop $InstallOutput
  Call un.NormalizeHelperExit

  ${If} $InstallResult == "0"
    Goto uninstall_success
  ${ElseIf} $InstallResult == "31"
    Goto uninstall_success
  ${ElseIf} $InstallResult == "20"
    IfSilent uninstall_failed
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "H3 Codec Pack ${PACK_VERSION} failed integrity validation.$\r$\n$\r$\nRemove only this exact corrupt version anyway?$\r$\n$\r$\n$InstallOutput" IDYES uninstall_force IDNO uninstall_cancelled
  ${Else}
    IfSilent uninstall_failed
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "H3 Codec Pack ${PACK_VERSION} could not be removed.$\r$\n$\r$\nClose LatentDeck, LatentPlayer, and Codec Pack workers, then choose Retry.$\r$\n$\r$\n$InstallOutput" IDRETRY uninstall_retry IDCANCEL uninstall_cancelled
  ${EndIf}

uninstall_force:
  nsExec::ExecToStack '"$PLUGINSDIR\${HELPER_FILE}" --local-app-data "$LOCALAPPDATA" --program-data "$ProgramDataRoot" uninstall --version "${PACK_VERSION}" --remove-corrupt'
  Pop $InstallResult
  Pop $InstallOutput
  Call un.NormalizeHelperExit
  ${If} $InstallResult == "0"
    Goto uninstall_success
  ${ElseIf} $InstallResult == "31"
    Goto uninstall_success
  ${EndIf}
  Goto uninstall_failed

uninstall_maintenance_root_unsafe:
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The fixed Codec Pack maintenance path contains an inaccessible, non-directory, or reparse-point component. No uninstall changes were made."
  SetErrorLevel 40
  Abort

uninstall_helper_extract_failed:
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "The uninstaller could not prepare its embedded lifecycle helper. No Codec Pack files or Windows maintenance data were changed."
  SetErrorLevel 70
  Abort

uninstall_cancelled:
  SetErrorLevel 1
  Abort

uninstall_failed:
  DetailPrint "$InstallOutput"
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "H3 Codec Pack uninstall failed (code $InstallResult).$\r$\n$\r$\n$InstallOutput"
  SetErrorLevel $InstallResult
  Abort

uninstall_success:
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_STAGE}" uninstall_maintenance_failed
  !insertmacro RemoveKnownMaintenanceTree "${MAINTENANCE_BACKUP}" uninstall_maintenance_failed
  ClearErrors
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  Delete "$INSTDIR\INSTALLER_NSIS_COPYING.txt"
  Delete "$INSTDIR\INSTALLER_RUST_LICENSES.txt"
  Delete "$INSTDIR\installer-SBOM.cdx.json"
  Delete "$INSTDIR\install-metadata.json"
  IfErrors uninstall_maintenance_failed

  ClearErrors
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  IfErrors uninstall_maintenance_failed
  StrCpy $MaintenanceRegistryRemoved "1"

  ClearErrors
  Delete /REBOOTOK "$INSTDIR\${HELPER_FILE}"
  Delete /REBOOTOK "$INSTDIR\Uninstall.exe"
  RMDir /REBOOTOK "$INSTDIR"
  IfErrors uninstall_maintenance_failed

  ; Empty shared parents are optional cleanup; other installed versions keep them.
  ClearErrors
  RMDir "$LOCALAPPDATA\LatentDeck\CodecPackMaintenance\${PACK_ID}"
  RMDir "$LOCALAPPDATA\LatentDeck\CodecPackMaintenance"
  SetErrorLevel 0
  Goto uninstall_done

uninstall_maintenance_failed:
  DetailPrint "The exact pack was removed, but Windows maintenance cleanup did not finish."
  ${If} $MaintenanceRegistryRemoved == "1"
    IfSilent +2
      MessageBox MB_OK|MB_ICONSTOP "H3 Codec Pack ${PACK_VERSION} and its Installed Apps entry were removed, but maintenance files could not be cleaned completely. Run the exact setup again beside its matching ZIP to repair maintenance, then uninstall once more."
    SetErrorLevel 60
    Abort
  ${EndIf}
  IfSilent +2
    MessageBox MB_OK|MB_ICONSTOP "H3 Codec Pack ${PACK_VERSION} was removed, but its Windows maintenance entry or files could not be cleaned completely. Run this uninstaller again after closing other maintenance processes."
  SetErrorLevel 60
  Abort

uninstall_done:
SectionEnd
