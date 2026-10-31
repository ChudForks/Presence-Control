Unicode true
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor /SOLID lzma
SetCompressorDictSize 32

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
!include "${PAYLOAD_DIR}\payload.nsh"

!define PRODUCT "Presence Control"
!ifndef SHORTCUT
!define SHORTCUT "Presence Control (Rust)"
!endif
!ifndef APP_KEY
!define APP_KEY "Software\Presence Control\Rust Slint"
!endif
!ifndef UNINSTALL_KEY
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PresenceControl.RustSlint"
!endif
!ifndef SETUP_MUTEX
!define SETUP_MUTEX "Local\PresenceControlRustSlintSetup"
!endif
!ifndef DEFAULT_INSTALL_DIR
!define DEFAULT_INSTALL_DIR "$LOCALAPPDATA\Programs\Presence Control Rust"
!endif

Name "${PRODUCT}"
OutFile "${INSTALLER_FILE}"
InstallDir ""
BrandingText "Presence Control - Native Rust Studio"
ShowInstDetails show
ShowUninstDetails show
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT}"
VIAddVersionKey /LANG=1033 "FileDescription" "Presence Control Windows installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Presence Control contributors"

!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "Install the native Rust version of Presence Control for your Windows account.$\r$\n$\r$\nNo administrator privileges are required. Your saved profiles and activity history are kept when upgrading or uninstalling."
!define MUI_FINISHPAGE_RUN "$INSTDIR\presence-control.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Presence Control"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "Remove Presence Control from this computer? Saved profiles, settings, activity history, and archives will be kept."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

Var SetupMutex

; Serialize setup and uninstall without terminating a running application.
!macro SetupInit PREFIX
Function ${PREFIX}.onInit
  SetShellVarContext current
  SetRegView 64
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "Presence Control requires 64-bit Windows 10 or later." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "Presence Control requires Windows 10 or later." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "${SETUP_MUTEX}") p .r0 ?e'
  Pop $1
  StrCpy $SetupMutex $0
  ${If} $0 == 0
  ${OrIf} $1 == 183
    MessageBox MB_OK|MB_ICONSTOP "Another Presence Control setup is running. Close it and try again." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  !if "${PREFIX}" == ""
    Call SetDefaultDirectory
  !endif
FunctionEnd
!macroend

!insertmacro SetupInit ""
!insertmacro SetupInit "un"

; Preserve /D= overrides, otherwise reuse the registered installation directory.
Function SetDefaultDirectory
  ${If} $INSTDIR == ""
    ReadRegStr $INSTDIR HKCU "${APP_KEY}" "InstallLocation"
    ${If} $INSTDIR == ""
      StrCpy $INSTDIR "${DEFAULT_INSTALL_DIR}"
    ${EndIf}
  ${EndIf}
FunctionEnd

; Opening with write access fails for an executable mapped by a running process.
; The handle never writes bytes. Silent setup fails instead of killing the app.
!macro CheckWritable PREFIX
Function ${PREFIX}CheckWritable
  IfFileExists "$INSTDIR\presence-control.exe" 0 done
  retry:
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\presence-control.exe", i 0x40000000, i 7, p 0, i 3, i 0, p 0) p .r0'
    ${If} $0 == -1
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Quit Presence Control from its system tray menu before continuing. If it has already exited, check that you can write to this installation folder." /SD IDCANCEL IDRETRY retry
      SetErrorLevel 2
      Abort
    ${EndIf}
    System::Call 'kernel32::CloseHandle(p r0)'
  done:
FunctionEnd
!macroend
!insertmacro CheckWritable ""
!insertmacro CheckWritable "un."

Section "Presence Control (required)" MainSection
  SectionIn RO
  Call SetDefaultDirectory
  Call CheckWritable
  SetOutPath "$INSTDIR"
  !insertmacro InstallPayload
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${SHORTCUT}.lnk" "$INSTDIR\presence-control.exe"
  WriteRegStr HKCU "${APP_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Presence Control (Rust)"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Presence Control"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\presence-control.exe,0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
SectionEnd

Section /o "Desktop shortcut" DesktopSection
  CreateShortcut "$DESKTOP\${SHORTCUT}.lnk" "$INSTDIR\presence-control.exe"
SectionEnd

Section "Uninstall"
  Call un.CheckWritable
  !insertmacro UninstallPayload
  Delete "$INSTDIR\Uninstall.exe"
  ; A copied older uninstaller must not remove another installation's shortcuts.
  ReadRegStr $0 HKCU "${APP_KEY}" "InstallLocation"
  ${If} $0 == $INSTDIR
    Delete "$SMPROGRAMS\${SHORTCUT}.lnk"
    Delete "$DESKTOP\${SHORTCUT}.lnk"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "${APP_KEY}"
    DeleteRegKey /ifempty HKCU "Software\Presence Control"
  ${EndIf}
  ; Never recursively delete the install directory or touch application data.
  RMDir "$INSTDIR"
SectionEnd
