; Per-user installer for Riven Launcher; no administrator rights needed, so the app can update itself.
;
;   makensis /DVERSION=<x.y.z> /DEXE=<path to riven.exe> /DOUT=<setup.exe> packaging\windows\riven.nsi
;
; The updater runs the new installer with /S over the old install; it finds the folder in the registry.

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true

!include "MUI2.nsh"
!include "x64.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "..\..\target\release\riven.exe"
!endif
!ifndef OUT
  !define OUT "Riven-Setup-x86_64.exe"
!endif

!define NAME      "Riven Launcher"
!define PUBLISHER "BX Team"
!define REGKEY    "Software\BX-Team\Riven"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Riven"
!define CLASSES   "Software\Classes"

Name "${NAME}"
BrandingText "${NAME} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\Riven"
InstallDirRegKey HKCU "${REGKEY}" "InstallDir"
RequestExecutionLevel user

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName"     "${NAME}"
VIAddVersionKey "CompanyName"     "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion"     "${VERSION}"
VIAddVersionKey "ProductVersion"  "${VERSION}"
VIAddVersionKey "LegalCopyright"  "BX Team, GPL-3.0-or-later"

!define MUI_ICON   "..\..\assets\brand\riven.ico"
!define MUI_UNICON "..\..\assets\brand\riven.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\riven.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Riven Launcher needs 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

Section "Riven Launcher" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "/oname=riven.exe" "${EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\riven.exe"
  IfSilent +2
  CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\riven.exe"

  WriteRegStr HKCU "${CLASSES}\riven" "" "URL:Riven pack link"
  WriteRegStr HKCU "${CLASSES}\riven" "URL Protocol" ""
  WriteRegStr HKCU "${CLASSES}\riven\DefaultIcon" "" "$INSTDIR\riven.exe,0"
  WriteRegStr HKCU "${CLASSES}\riven\shell\open\command" "" '"$INSTDIR\riven.exe" "%1"'

  WriteRegStr HKCU "${CLASSES}\.riven" "" "Riven.Pack"
  WriteRegStr HKCU "${CLASSES}\Riven.Pack" "" "Riven pack"
  WriteRegStr HKCU "${CLASSES}\Riven.Pack\DefaultIcon" "" "$INSTDIR\riven.exe,0"
  WriteRegStr HKCU "${CLASSES}\Riven.Pack\shell\open\command" "" '"$INSTDIR\riven.exe" "%1"'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'

  WriteRegStr HKCU "${REGKEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName"     "${NAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher"       "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon"     "$INSTDIR\riven.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "URLInfoAbout"    "https://github.com/BX-Team/Riven"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
SectionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

; Instances, accounts and settings live in %APPDATA%\Riven and stay.
Section "Uninstall"
  Delete "$INSTDIR\riven.exe"
  Delete "$INSTDIR\riven.exe.old"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$DESKTOP\${NAME}.lnk"

  DeleteRegKey HKCU "${CLASSES}\riven"
  DeleteRegKey HKCU "${CLASSES}\Riven.Pack"
  ReadRegStr $0 HKCU "${CLASSES}\.riven" ""
  StrCmp $0 "Riven.Pack" 0 +2
    DeleteRegKey HKCU "${CLASSES}\.riven"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'

  DeleteRegKey HKCU "${UNINSTKEY}"
  DeleteRegKey HKCU "${REGKEY}"
SectionEnd
