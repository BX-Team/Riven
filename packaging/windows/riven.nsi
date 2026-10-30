; Per-user installer for Riven Launcher; no administrator rights needed, so the app can update itself.
;
;   makensis /DVERSION=<x.y.z> /DEXE=<riven.exe> /DCLI=<riven-cli riven.exe> /DPLUGINS=<folder with EnVar.dll> /DOUT=<setup.exe> packaging\windows\riven.nsi
;
; The updater runs the new installer with /S over the old install; it finds the folder and the
; command-line and completion choices in the registry.
;
; The command line is the console build installed as riven.com next to riven.exe: PATHEXT tries .COM
; before .EXE, so `riven` in a terminal gets the console one while shortcuts open the window.

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true

!include "MUI2.nsh"
!include "Sections.nsh"
!include "x64.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "..\..\target\release\riven.exe"
!endif
!ifndef CLI
  !define CLI "..\..\target\cli\release\riven.exe"
!endif
!ifndef OUT
  !define OUT "Riven-Setup-x86_64.exe"
!endif
!ifdef PLUGINS
  !addplugindir /x86-unicode "${PLUGINS}"
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
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"

LangString CLI_NAME ${LANG_ENGLISH} "Command line (riven in PATH)"
LangString CLI_NAME ${LANG_RUSSIAN} "Командная строка (riven в PATH)"
LangString CLI_ABOUT ${LANG_ENGLISH} "Run riven install, riven add and the other commands from cmd and PowerShell."
LangString CLI_ABOUT ${LANG_RUSSIAN} "Команды riven install, riven add и другие в cmd и PowerShell."
LangString PS_NAME ${LANG_ENGLISH} "Tab completion in PowerShell"
LangString PS_NAME ${LANG_RUSSIAN} "Автодополнение в PowerShell"
LangString PS_ABOUT ${LANG_ENGLISH} "Adds a line to your PowerShell profile so Tab completes riven commands and pack entries. Needs PowerShell 7, or Windows PowerShell with scripts allowed."
LangString PS_ABOUT ${LANG_RUSSIAN} "Добавляет строку в профиль PowerShell: Tab дополняет команды riven и записи пака. Нужен PowerShell 7 или Windows PowerShell с разрешёнными скриптами."
LangString MAIN_ABOUT ${LANG_ENGLISH} "The launcher itself."
LangString MAIN_ABOUT ${LANG_RUSSIAN} "Сам лаунчер."

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

Section /o "$(CLI_NAME)" SecCli
  SetOutPath "$INSTDIR"
  ; A running riven.com cannot be overwritten but can be renamed.
  Delete "$INSTDIR\riven.com.old"
  Rename "$INSTDIR\riven.com" "$INSTDIR\riven.com.old"
  File "/oname=riven.com" "${CLI}"
  Delete "$INSTDIR\riven.com.old"
  EnVar::SetHKCU
  EnVar::AddValue "Path" "$INSTDIR"
  Pop $0
  WriteRegDWORD HKCU "${REGKEY}" "Cli" 1
SectionEnd

Section /o "$(PS_NAME)" SecPs
  nsExec::Exec '"$INSTDIR\riven.com" powershell-profile'
  Pop $0
  WriteRegDWORD HKCU "${REGKEY}" "PowerShell" 1
SectionEnd

Section "-Drop what was unticked"
  ${IfNot} ${SectionIsSelected} ${SecPs}
    ${If} ${FileExists} "$INSTDIR\riven.com"
      nsExec::Exec '"$INSTDIR\riven.com" powershell-profile --remove'
      Pop $0
    ${EndIf}
    WriteRegDWORD HKCU "${REGKEY}" "PowerShell" 0
  ${EndIf}
  ${IfNot} ${SectionIsSelected} ${SecCli}
    Delete "$INSTDIR\riven.com"
    EnVar::SetHKCU
    EnVar::DeleteValue "Path" "$INSTDIR"
    Pop $0
    WriteRegDWORD HKCU "${REGKEY}" "Cli" 0
  ${EndIf}
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(MAIN_ABOUT)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCli} $(CLI_ABOUT)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPs} $(PS_ABOUT)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; After the sections, so it can name them. A silent update keeps the earlier choice.
Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Riven Launcher needs 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
  ReadRegDWORD $0 HKCU "${REGKEY}" "Cli"
  ${If} $0 = 1
    !insertmacro SelectSection ${SecCli}
  ${EndIf}
  ReadRegDWORD $0 HKCU "${REGKEY}" "PowerShell"
  ${If} $0 = 1
    !insertmacro SelectSection ${SecPs}
  ${EndIf}
FunctionEnd

; Completion runs riven.com, so it comes and goes with the command line.
Function .onSelChange
  ${If} $0 = ${SecPs}
  ${AndIf} ${SectionIsSelected} ${SecPs}
    !insertmacro SelectSection ${SecCli}
  ${ElseIf} $0 = ${SecCli}
  ${AndIfNot} ${SectionIsSelected} ${SecCli}
    !insertmacro UnselectSection ${SecPs}
  ${EndIf}
FunctionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

; Instances, accounts and settings live in %APPDATA%\Riven and stay.
Section "Uninstall"
  ${If} ${FileExists} "$INSTDIR\riven.com"
    nsExec::Exec '"$INSTDIR\riven.com" powershell-profile --remove'
    Pop $0
  ${EndIf}
  EnVar::SetHKCU
  EnVar::DeleteValue "Path" "$INSTDIR"
  Pop $0
  Delete "$INSTDIR\riven.exe"
  Delete "$INSTDIR\riven.exe.old"
  Delete "$INSTDIR\riven.com"
  Delete "$INSTDIR\riven.com.old"
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
