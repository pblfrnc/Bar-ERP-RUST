; Instalador do BarERP Pro nativo (Rust + Slint).
;
; Gerar (Mac ou Windows), depois do build release do Windows:
;   makensis rust/installer/barerp.nsi
; Sai em rust/target/installer/BarERP-Instalador-Windows.exe
;
; Dois modos, escolhidos na primeira página:
; - "Todos os usuários": pede administrador (UAC), instala em Arquivos de Programas e SUBSTITUI a versão
;   Electron (desinstala a antiga, guardando antes uma cópia do banco);
; - "Só para mim": sem administrador, instala em %LOCALAPPDATA%\Programs e não mexe na versão Electron.
;
; O banco (bar.db), os backups e as configurações ficam em %APPDATA%\BarERP Pro, a pasta do USUÁRIO,
; fora da pasta do programa. Nem este instalador nem o desinstalador apagam nada lá; a desinstalação da
; versão Electron roda com --updated, que no electron-builder nunca apaga o AppData.
;
; O que o atualizador (crates/barerp-updater) espera daqui:
; - o nome BarERP-Instalador-Windows.exe;
; - aberto sem argumentos (de %TEMP%\BarERP-Instalador-Update.exe), instala por cima no mesmo modo da
;   instalação existente, espera ou encerra o barerp.exe ainda aberto e reabre o BarERP no final.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!define PRODUCT "BarERP Pro"
!define EXE "barerp.exe"
!define FOLDER "BarERP Nativo"
!define REGKEY "Software\BarERP Nativo"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\BarERP Nativo"
; Versão Electron (package.json: productName "BarERP Pro", appId com.barerp.desktop)
!define OLD_EXE "BarERP Pro.exe"
!define OLD_UNINSTALLER "Uninstall BarERP Pro.exe"
!ifndef VERSION
  !define VERSION "2.0.0"
!endif
!ifndef SOURCE_EXE
  !define SOURCE_EXE "../target/x86_64-pc-windows-msvc/release/barerp.exe"
!endif

Name "${PRODUCT}"
OutFile "../target/installer/BarERP-Instalador-Windows.exe"
InstallDir "$LOCALAPPDATA\Programs\${FOLDER}"
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1046 "ProductName" "${PRODUCT}"
VIAddVersionKey /LANG=1046 "FileDescription" "Instalador do ${PRODUCT}"
VIAddVersionKey /LANG=1046 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1046 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1046 "CompanyName" "Pablo Franco"
VIAddVersionKey /LANG=1046 "LegalCopyright" "Pablo Franco"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"

!define MUI_ICON "../crates/barerp-app/assets/icon.ico"
!define MUI_UNICON "../crates/barerp-app/assets/icon.ico"
!define MUI_ABORTWARNING

Page custom ModePage ModePageLeave
; Atualização (já instalado): pula a escolha da pasta e instala direto por cima
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfUpdate
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
; Atualização: sem a página final, reabre o app direto (o operador só clicou em "Instalar" no app)
!define MUI_PAGE_CUSTOMFUNCTION_PRE FinishPre
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Abrir o ${PRODUCT}"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "PortugueseBR"

Var AllUsers   ; "1" = todos os usuários (Arquivos de Programas, HKLM, atalhos públicos)
Var IsUpdate   ; "1" = já existe o BarERP nativo instalado: instala por cima no mesmo modo
Var Elevated   ; "1" = esta cópia foi reaberta como administrador (/ALLUSERS)
Var RadioAll
Var RadioUser

!macro IsAdmin RESULT
  UserInfo::GetAccountType
  Pop ${RESULT}
!macroend

; Abre o app como o usuário da sessão, mesmo quando o instalador roda como administrador: o app lê o
; banco do %APPDATA% de quem abre, e rodar elevado abriria o AppData de outra conta
Function LaunchApp
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE}"'
FunctionEnd

Function .onInit
  SetRegView 64
  StrCpy $AllUsers "0"
  StrCpy $IsUpdate "0"
  StrCpy $Elevated "0"
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/ALLUSERS" $R1
  ${IfNot} ${Errors}
    StrCpy $AllUsers "1"
    StrCpy $Elevated "1"
  ${EndIf}

  ReadRegStr $0 HKLM "${REGKEY}" "InstallDir"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\${EXE}"
    StrCpy $AllUsers "1"
    StrCpy $IsUpdate "1"
    StrCpy $INSTDIR $0
  ${Else}
    ReadRegStr $0 HKCU "${REGKEY}" "InstallDir"
    ${If} $0 != ""
    ${AndIf} ${FileExists} "$0\${EXE}"
    ${AndIf} $Elevated == "0"
      StrCpy $IsUpdate "1"
      StrCpy $INSTDIR $0
    ${EndIf}
  ${EndIf}

  ${If} $AllUsers == "1"
    ${If} $IsUpdate == "0"
      StrCpy $INSTDIR "$PROGRAMFILES64\${FOLDER}"
    ${EndIf}
    !insertmacro IsAdmin $1
    ${If} $1 != "Admin"
      ${If} $Elevated == "1"
        MessageBox MB_ICONSTOP "A instalação para todos os usuários precisa de um administrador."
        Quit
      ${EndIf}
      ; Atualização de uma instalação para todos os usuários: reabre pedindo administrador
      ClearErrors
      ExecShell "runas" "$EXEPATH" "/ALLUSERS $R0"
      ${If} ${Errors}
        MessageBox MB_ICONEXCLAMATION "A atualização não foi feita: ela precisa da permissão de administrador.$\r$\nO ${PRODUCT} continua na versão atual."
        Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE}"'
      ${EndIf}
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

Function SkipIfUpdate
  ${If} $IsUpdate == "1"
    Abort
  ${EndIf}
FunctionEnd

Function FinishPre
  ${If} $IsUpdate == "1"
    Call LaunchApp
    Abort
  ${EndIf}
FunctionEnd

Function ModePage
  ${If} $IsUpdate == "1"
  ${OrIf} $Elevated == "1"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Tipo de instalação" "Escolha para quem o ${PRODUCT} será instalado."
  nsDialogs::Create 1018
  Pop $0
  ${NSD_CreateRadioButton} 0 0u 100% 12u "Para todos os usuários: substitui a instalação anterior (pede administrador)"
  Pop $RadioAll
  ${NSD_CreateLabel} 12u 14u 95% 28u "Instala em Arquivos de Programas e remove a versão anterior do ${PRODUCT}. O banco de dados, os backups e as configurações são mantidos, e antes uma cópia do banco é guardada."
  Pop $0
  ${NSD_CreateRadioButton} 0 50u 100% 12u "Só para mim: sem administrador"
  Pop $RadioUser
  ${NSD_CreateLabel} 12u 64u 95% 28u "Instala na pasta do seu usuário. A versão anterior, se houver, continua instalada; o atalho ${PRODUCT} passa a abrir esta versão."
  Pop $0
  ${NSD_Check} $RadioAll
  nsDialogs::Show
FunctionEnd

Function ModePageLeave
  ${NSD_GetState} $RadioAll $0
  ${If} $0 == ${BST_CHECKED}
    !insertmacro IsAdmin $1
    ${If} $1 == "Admin"
      StrCpy $AllUsers "1"
      StrCpy $INSTDIR "$PROGRAMFILES64\${FOLDER}"
      Return
    ${EndIf}
    ClearErrors
    ExecShell "runas" "$EXEPATH" "/ALLUSERS"
    ${If} ${Errors}
      MessageBox MB_ICONEXCLAMATION "Sem a permissão de administrador não dá para instalar para todos os usuários.$\r$\nTente de novo ou escolha $\"Só para mim$\"."
      Abort
    ${EndIf}
    Quit
  ${EndIf}
  StrCpy $AllUsers "0"
  StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${FOLDER}"
FunctionEnd

; Espera até 10 s o executável ser liberado (o atualizador fecha o app logo depois de abrir o instalador)
; e só então encerra à força o que tiver ficado aberto
!macro CloseApp PATH NAME
  ${If} ${FileExists} "${PATH}"
    StrCpy $1 0
    ${Do}
      ClearErrors
      FileOpen $0 "${PATH}" a
      ${IfNot} ${Errors}
        FileClose $0
        ${Break}
      ${EndIf}
      ${If} $1 == 20
        DetailPrint "Fechando o ${PRODUCT} que ainda estava aberto..."
        nsExec::Exec 'taskkill /F /IM "${NAME}"'
        Pop $0
        Sleep 1500
        ${Break}
      ${EndIf}
      IntOp $1 $1 + 1
      Sleep 500
    ${Loop}
  ${EndIf}
!macroend

; O atalho "BarERP Pro" tem o mesmo nome nas duas versões: só apaga se ele abre o executável TARGET
!macro DeleteShortcutIfTarget LNK TARGET
  ${If} ${FileExists} "${LNK}"
    nsExec::ExecToStack `powershell -NoProfile -NonInteractive -Command "(New-Object -ComObject WScript.Shell).CreateShortcut('${LNK}').TargetPath"`
    Pop $0
    Pop $1
    ; a saída do PowerShell termina com quebra de linha
    StrCpy $2 "${TARGET}"
    StrLen $3 $2
    StrCpy $1 $1 $3
    ${If} $0 == "0"
    ${AndIf} $1 == $2
      Delete "${LNK}"
    ${EndIf}
  ${EndIf}
!macroend

; Cópia de segurança do banco antes de remover a versão Electron (uma vez só; não sobrescreve)
!macro BackupDb FOLDERNAME
  ${If} ${FileExists} "$APPDATA\${FOLDERNAME}\bar.db"
  ${AndIfNot} ${FileExists} "$APPDATA\${FOLDERNAME}\copia-antes-da-troca\bar.db"
    CreateDirectory "$APPDATA\${FOLDERNAME}\copia-antes-da-troca"
    CopyFiles /SILENT "$APPDATA\${FOLDERNAME}\bar.db*" "$APPDATA\${FOLDERNAME}\copia-antes-da-troca"
    DetailPrint "Cópia do banco guardada em $APPDATA\${FOLDERNAME}\copia-antes-da-troca"
  ${EndIf}
!macroend

; Procura a versão Electron em ROOT (HKLM ou HKCU), na vista do registro atual, e desinstala
!macro RemoveElectronIn ROOT
  StrCpy $5 0
  ${Do}
    EnumRegKey $6 ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall" $5
    ${If} $6 == ""
      ${Break}
    ${EndIf}
    IntOp $5 $5 + 1
    ${If} $6 == "BarERP Nativo"
      ${Continue}
    ${EndIf}
    ReadRegStr $7 ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$6" "DisplayName"
    StrCpy $8 $7 10
    ${If} $8 != "BarERP Pro"
      ${Continue}
    ${EndIf}
    ; O electron-builder não grava InstallLocation nesta chave: a pasta sai do UninstallString,
    ; '"<pasta>\Uninstall BarERP Pro.exe" /allusers' (ou /currentuser)
    ReadRegStr $9 ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$6" "UninstallString"
    Push $9
    Call QuotedExeDir
    Pop $8
    ${If} $8 == ""
    ${OrIfNot} ${FileExists} "$8\${OLD_UNINSTALLER}"
      ${Continue}
    ${EndIf}
    DetailPrint "Removendo a versão anterior: $8"
    !insertmacro CloseApp "$8\${OLD_EXE}" "${OLD_EXE}"
    ; --updated: o desinstalador do electron-builder não apaga o AppData; _?= roda no lugar e espera
    ExecWait '$9 /S --updated _?=$8' $0
    Delete "$8\${OLD_UNINSTALLER}"
    RMDir "$8"
    !insertmacro DeleteShortcutIfTarget "$DESKTOP\${PRODUCT}.lnk" "$8\${OLD_EXE}"
    !insertmacro DeleteShortcutIfTarget "$SMPROGRAMS\${PRODUCT}.lnk" "$8\${OLD_EXE}"
    ${IfNot} ${FileExists} "$8\${OLD_EXE}"
      DeleteRegKey ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$6"
    ${EndIf}
    ; as chaves mudaram: recomeça a lista
    StrCpy $5 0
  ${Loop}
!macroend

; '"C:\pasta\programa.exe" /args' -> 'C:\pasta' (vazio se não começar com aspas)
Function QuotedExeDir
  Exch $R0
  Push $R1
  Push $R2
  StrCpy $R1 $R0 1
  ${If} $R1 != '"'
    StrCpy $R0 ""
  ${Else}
    StrCpy $R0 $R0 "" 1
    StrCpy $R2 0
    ${Do}
      StrCpy $R1 $R0 1 $R2
      ${If} $R1 == ""
        StrCpy $R0 ""
        ${Break}
      ${EndIf}
      ${If} $R1 == '"'
        StrCpy $R0 $R0 $R2
        ${GetParent} $R0 $R0
        ${Break}
      ${EndIf}
      IntOp $R2 $R2 + 1
    ${Loop}
  ${EndIf}
  Pop $R2
  Pop $R1
  Exch $R0
FunctionEnd

Function ReplaceElectron
  ; Fecha as duas versões antes de copiar o banco: com o app gravando, a cópia sairia corrompida
  nsExec::Exec 'taskkill /IM "${OLD_EXE}"'
  Pop $0
  !insertmacro CloseApp "$INSTDIR\${EXE}" "${EXE}"
  Sleep 2000
  nsExec::Exec 'taskkill /F /IM "${OLD_EXE}"'
  Pop $0
  ; O banco é do usuário da sessão (%APPDATA% dele), mesmo com o instalador como administrador
  SetShellVarContext current
  !insertmacro BackupDb "BarERP Pro"
  !insertmacro BackupDb "bar-erp"
  !insertmacro BackupDb "BarERP"
  !insertmacro RemoveElectronIn HKCU
  SetShellVarContext all
  SetRegView 64
  !insertmacro RemoveElectronIn HKLM
  SetRegView 32
  !insertmacro RemoveElectronIn HKLM
  SetRegView 64
FunctionEnd

Section "Instalar"
  ${If} $AllUsers == "1"
    Call ReplaceElectron
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
  !insertmacro CloseApp "$INSTDIR\${EXE}" "${EXE}"
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File "/oname=${EXE}" "${SOURCE_EXE}"
  WriteUninstaller "$INSTDIR\Desinstalar.exe"

  CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0

  ; SHCTX = HKLM (todos os usuários) ou HKCU (só para mim), conforme o SetShellVarContext
  WriteRegStr SHCTX "${REGKEY}" "InstallDir" "$INSTDIR"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "Pablo Franco"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" '"$INSTDIR\Desinstalar.exe"'
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" 1

  ; Sem janela (/S): reabre sozinho, como o instalador do Electron fazia
  ${If} ${Silent}
    Call LaunchApp
  ${EndIf}
  ; Atualização: avança sozinho da barra de progresso para o fim (FinishPre reabre o app)
  ${If} $IsUpdate == "1"
    SetAutoClose true
  ${EndIf}
SectionEnd

Var UnAllUsers

Function un.onInit
  SetRegView 64
  StrCpy $UnAllUsers "0"
  ReadRegStr $0 HKLM "${REGKEY}" "InstallDir"
  ${If} $0 == $INSTDIR
    StrCpy $UnAllUsers "1"
    !insertmacro IsAdmin $1
    ${If} $1 != "Admin"
      ClearErrors
      ExecShell "runas" "$INSTDIR\Desinstalar.exe"
      ${If} ${Errors}
        MessageBox MB_ICONEXCLAMATION "A desinstalação precisa da permissão de administrador."
      ${EndIf}
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

; Remove só o programa, os atalhos e o registro; o banco e os backups em %APPDATA% ficam
Section "Uninstall"
  ${If} $UnAllUsers == "1"
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
  !insertmacro CloseApp "$INSTDIR\${EXE}" "${EXE}"
  !insertmacro DeleteShortcutIfTarget "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${EXE}"
  !insertmacro DeleteShortcutIfTarget "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\Desinstalar.exe"
  RMDir "$INSTDIR"
  DeleteRegKey SHCTX "${UNINSTKEY}"
  DeleteRegKey SHCTX "${REGKEY}"
SectionEnd
