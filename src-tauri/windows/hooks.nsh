!ifndef FOLDSENTINEL_UNINSTALL_HOOKS
!define FOLDSENTINEL_UNINSTALL_HOOKS

!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"
!include "Win\COM.nsh"
!define /ifndef SLGP_RAWPATH 0x4

Var FoldSentinelUninstallHelper
Var FoldSentinelUninstallParentPid
Var FoldSentinelUninstallExitCode
Var FoldSentinelUninstallUnattended
Var FoldSentinelUninstallPrepared
Var FoldSentinelUninstallForced
Var FoldSentinelUninstallRegistryCode
Var FoldSentinelInstallServiceSelected
Var FoldSentinelInstallServiceCheckbox
Var FoldSentinelInstallServiceResult
Var FoldSentinelInstallFinishText
!define MUI_FINISHPAGE_TEXT "$FoldSentinelInstallFinishText"

; hooks 在 Tauri 标准页面和产品常量之前载入。独立选项页仅使用本产品
; 固定文案，不替换上游整套模板；默认不安装，静默/被动安装也不提权。
Page custom FoldSentinelServicePage FoldSentinelServiceLeave

Function FoldSentinelServicePage
  IfSilent FoldSentinelServicePageSkip
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/P" $1
  ${IfNot} ${Errors}
    Goto FoldSentinelServicePageSkip
  ${EndIf}
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    MessageBox MB_OK|MB_ICONSTOP "Cannot create the service installation page. Setup will stop."
    Quit
  ${EndIf}
  ${If} $LANGUAGE == 2052
    !insertmacro MUI_HEADER_TEXT "可选持续防护服务" "按需安装；仅服务部署需要管理员授权。"
    ${NSD_CreateLabel} 0 0 100% 42u "SYSTEM 服务提供密码访问、到期重锁和后台持续防护。主程序按当前用户安装；不安装服务仍可使用本地保护与加密功能。"
    Pop $0
    ${NSD_CreateCheckbox} 0 52u 100% 28u "安装 SYSTEM 持续防护服务（需要管理员权限）"
    Pop $FoldSentinelInstallServiceCheckbox
    ${NSD_CreateLabel} 0 92u 100% 58u "选择后，安装程序将请求 Windows 管理员授权，并配置服务自动启动。已有兼容服务会保留；恢复资料不会删除。取消授权时只完成主程序安装。"
    Pop $0
  ${Else}
    !insertmacro MUI_HEADER_TEXT "Optional protection service" "Only service deployment requires administrator approval."
    ${NSD_CreateLabel} 0 0 100% 42u "The SYSTEM service provides password access, timed relocking and background protection. The app installs for the current user. Local protection and encryption remain available without the service."
    Pop $0
    ${NSD_CreateCheckbox} 0 52u 100% 28u "Install the SYSTEM protection service (administrator access required)"
    Pop $FoldSentinelInstallServiceCheckbox
    ${NSD_CreateLabel} 0 92u 100% 58u "When selected, setup requests Windows administrator approval and configures automatic service startup. Compatible existing services and recovery records are retained. Canceling approval installs the app only."
    Pop $0
  ${EndIf}
  ${If} $FoldSentinelInstallServiceSelected == ${BST_CHECKED}
    ${NSD_Check} $FoldSentinelInstallServiceCheckbox
  ${Else}
    ${NSD_Uncheck} $FoldSentinelInstallServiceCheckbox
  ${EndIf}
  nsDialogs::Show
  Return
FoldSentinelServicePageSkip:
  StrCpy $FoldSentinelInstallServiceSelected 0
  Abort
FunctionEnd

Function FoldSentinelServiceLeave
  ${NSD_GetState} $FoldSentinelInstallServiceCheckbox $FoldSentinelInstallServiceSelected
FunctionEnd

; 把调用与交互反馈分开，使隔离夹具能核对真实 ExecWait 和退出码而不触发 UAC。
!macro FoldSentinelRunServiceInstall
  StrCpy $FoldSentinelInstallServiceResult 21
  ${If} ${FileExists} "$INSTDIR\foldsentinel-service.exe"
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --install-service' $FoldSentinelInstallServiceResult
    ${If} ${Errors}
      StrCpy $FoldSentinelInstallServiceResult 21
    ${EndIf}
  ${EndIf}
  ${If} $FoldSentinelInstallServiceResult == 0
    DetailPrint "FoldSentinel: protection service installed or verified."
  ${Else}
    DetailPrint "FoldSentinel: app installed, but service setup did not complete (exit $FoldSentinelInstallServiceResult)."
    SetErrorLevel $FoldSentinelInstallServiceResult
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $LANGUAGE == 2052
    StrCpy $FoldSentinelInstallFinishText "主程序已安装。持续防护服务是可选组件；未选择安装时，仍可使用本地保护与加密功能。点击完成退出安装程序。"
  ${Else}
    StrCpy $FoldSentinelInstallFinishText "The app is installed. The protection service is optional; local protection and encryption remain available without it. Click Finish to close setup."
  ${EndIf}
  ; 不能让 /S、/P 或脚本设值绕过用户在选项页上的明确选择。
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode != 1
  ${AndIf} $FoldSentinelInstallServiceSelected == ${BST_CHECKED}
    !insertmacro FoldSentinelRunServiceInstall
    ${If} $FoldSentinelInstallServiceResult == 0
      ${If} $LANGUAGE == 2052
        StrCpy $FoldSentinelInstallFinishText "主程序已安装，持续防护服务已安装或验证可用。请在应用中核对服务连接与已登记项目。点击完成退出安装程序。"
      ${Else}
        StrCpy $FoldSentinelInstallFinishText "The app is installed, and the protection service was installed or verified available. Check service connection and registered items in the app. Click Finish to close setup."
      ${EndIf}
    ${Else}
      ${If} $LANGUAGE == 2052
        StrCpy $FoldSentinelInstallFinishText "主程序已安装，但持续防护服务安装未完成（错误码：$FoldSentinelInstallServiceResult）。请保留恢复资料，重新运行安装器并选择服务选项重试。点击完成退出安装程序。"
        ${If} $FoldSentinelInstallServiceResult == 1223
          MessageBox MB_OK|MB_ICONEXCLAMATION "主程序已安装，但你取消了管理员授权，持续防护服务未完成安装。可重新运行安装器并选择安装服务；现有恢复资料保留。"
        ${Else}
          MessageBox MB_OK|MB_ICONEXCLAMATION "主程序已安装，但持续防护服务未完成安装（错误码：$FoldSentinelInstallServiceResult）。请保留恢复资料，查看服务状态与安装指引后重试；不能视为服务已启用。"
        ${EndIf}
      ${Else}
        StrCpy $FoldSentinelInstallFinishText "The app is installed, but protection service setup did not complete (exit $FoldSentinelInstallServiceResult). Retain recovery records and run setup again with the service option to retry. Click Finish to close setup."
        ${If} $FoldSentinelInstallServiceResult == 1223
          MessageBox MB_OK|MB_ICONEXCLAMATION "The app is installed, but administrator approval was canceled and service setup did not complete. Run setup again and select the service option to retry. Recovery records are retained."
        ${Else}
          MessageBox MB_OK|MB_ICONEXCLAMATION "The app is installed, but service setup did not complete (exit $FoldSentinelInstallServiceResult). Retain recovery records and review service status and installation guidance before retrying. The service is not confirmed available."
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; Tauri 的 Delete / DeleteRegKey 不保证失败后返回非零。删除恢复资料前必须
; 独立核对已知程序与系统入口；未知文件和指向其他程序的快捷方式不删除。
!macro FoldSentinelVerifyRemovedFile PATH
  System::Call 'kernel32::GetFileAttributesW(w "${PATH}") i .r0 ?e'
  Pop $FoldSentinelUninstallRegistryCode
  ${If} $0 == -1
    ${If} $FoldSentinelUninstallRegistryCode != 2
    ${AndIf} $FoldSentinelUninstallRegistryCode != 3
      StrCpy $FoldSentinelUninstallExitCode 25
      Goto FoldSentinelUninstallFinishFailed
    ${EndIf}
  ${Else}
    DetailPrint "FoldSentinel: program file remains: ${PATH}"
    StrCpy $FoldSentinelUninstallExitCode 25
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
!macroend

!macro FoldSentinelOpenRegistryKey ROOT KEY
  !if "${ARCH}" == "x64"
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${KEY}", i 0, i 0x101, *p .r0) i .s'
  !else
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${KEY}", i 0, i 1, *p .r0) i .s'
  !endif
  Pop $FoldSentinelUninstallRegistryCode
!macroend

!macro FoldSentinelVerifyRemovedRegistryKey ROOT KEY
  !insertmacro FoldSentinelOpenRegistryKey ${ROOT} "${KEY}"
  ${If} $FoldSentinelUninstallRegistryCode == 0
    System::Call 'advapi32::RegCloseKey(p r0)'
    StrCpy $FoldSentinelUninstallRegistryCode 0
  ${EndIf}
  ${If} $FoldSentinelUninstallRegistryCode != 2
  ${AndIf} $FoldSentinelUninstallRegistryCode != 3
    DetailPrint "FoldSentinel: uninstall registration remains or cannot be checked."
    StrCpy $FoldSentinelUninstallExitCode 26
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
!macroend

!macro FoldSentinelVerifyRemovedRegistryValue ROOT KEY VALUE
  !insertmacro FoldSentinelOpenRegistryKey ${ROOT} "${KEY}"
  ${If} $FoldSentinelUninstallRegistryCode == 0
    System::Call 'advapi32::RegQueryValueExW(p r0, w "${VALUE}", p 0, p 0, p 0, p 0) i .s'
    Pop $FoldSentinelUninstallRegistryCode
    System::Call 'advapi32::RegCloseKey(p r0)'
  ${EndIf}
  ${If} $FoldSentinelUninstallRegistryCode != 2
  ${AndIf} $FoldSentinelUninstallRegistryCode != 3
    DetailPrint "FoldSentinel: autostart registration remains or cannot be checked."
    StrCpy $FoldSentinelUninstallExitCode 26
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
!macroend

!macro FoldSentinelVerifyRemovedShortcut PATH
  System::Call 'kernel32::GetFileAttributesW(w "${PATH}") i .r0 ?e'
  Pop $FoldSentinelUninstallRegistryCode
  ${If} $0 == -1
    ${If} $FoldSentinelUninstallRegistryCode != 2
    ${AndIf} $FoldSentinelUninstallRegistryCode != 3
      StrCpy $FoldSentinelUninstallExitCode 27
      Goto FoldSentinelUninstallFinishFailed
    ${EndIf}
  ${Else}
    ; 只有成功读取且目标属于其他程序的快捷方式才作为未知内容保留。
    ; COM/读取失败不能冒充“快捷方式不属于本软件”。
    StrCpy $0 0
    StrCpy $1 0
    StrCpy $2 ""
    StrCpy $5 0
    !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ".r3"
    ${If} $3 >= 0
    ${AndIf} $0 P<> 0
      ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}", .r1).r3'
      ${If} $3 >= 0
      ${AndIf} $1 P<> 0
        ${IPersistFile::Load} $1 '("${PATH}", ${STGM_READ}).r3'
        ${If} $3 >= 0
          ${IShellLink::GetPath} $0 '(.r2, ${NSIS_MAX_STRLEN}, 0, ${SLGP_RAWPATH}).r3'
          ${If} $3 >= 0
          ${AndIf} $2 != ""
          ${AndIf} $2 != "$INSTDIR\${MAINBINARYNAME}.exe"
            StrCpy $5 1
          ${EndIf}
        ${EndIf}
        ${IUnknown::Release} $1 ""
      ${EndIf}
      ${IUnknown::Release} $0 ""
    ${EndIf}
    ${If} $5 != 1
      DetailPrint "FoldSentinel: app shortcut remains or cannot be checked: ${PATH}"
      StrCpy $FoldSentinelUninstallExitCode 27
      Goto FoldSentinelUninstallFinishFailed
    ${EndIf}
  ${EndIf}
!macroend

; 只删除安装器写入的已知值；未知值和子键必须保留。系统返回“值/键不存在”
; 可视为已清理，权限等其他错误须令卸载收尾失败。
!macro FoldSentinelDeleteKnownRegistryValue ROOT KEY VALUE
  ; System 插件随 NSIS 运行在32位进程中，显式匹配目标注册表视图。
  !if "${ARCH}" == "x64"
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${KEY}", i 0, i 0x103, *p .r0) i .s'
  !else
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${KEY}", i 0, i 3, *p .r0) i .s'
  !endif
  Pop $FoldSentinelUninstallRegistryCode
  ${If} $FoldSentinelUninstallRegistryCode == 0
    System::Call 'advapi32::RegDeleteValueW(p r0, w "${VALUE}") i .s'
    Pop $FoldSentinelUninstallRegistryCode
    System::Call 'advapi32::RegCloseKey(p r0)'
  ${EndIf}
  ${If} $FoldSentinelUninstallRegistryCode != 0
  ${AndIf} $FoldSentinelUninstallRegistryCode != 2
  ${AndIf} $FoldSentinelUninstallRegistryCode != 3
    StrCpy $FoldSentinelUninstallExitCode 24
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
!macroend

; 此文件在 Tauri 定义 MAINBINARYNAME 等常量前载入，业务逻辑必须留在宏内。
!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $FoldSentinelUninstallPrepared 0
  StrCpy $FoldSentinelUninstallForced 0
  StrCpy $FoldSentinelUninstallUnattended 0
  ${If} ${Silent}
    StrCpy $FoldSentinelUninstallUnattended 1
  ${EndIf}
  ${If} $PassiveMode == 1
    StrCpy $FoldSentinelUninstallUnattended 1
  ${EndIf}

  ; 应用数据由 helper 按预检结果精确清理，禁止 Tauri 递归删除恢复材料。
  StrCpy $DeleteAppDataCheckboxState 0
  ; 安装器升级不调用本卸载段；直接传 /UPDATE 仍是实际卸载，须清除全部
  ; 程序快捷方式和自启动登记，不能把公开参数当作保留程序痕迹的授权。
  StrCpy $UpdateMode 0
  InitPluginsDir
  StrCpy $FoldSentinelUninstallHelper "$PLUGINSDIR\FoldSentinel-uninstall-helper.exe"
  System::Call 'kernel32::GetCurrentProcessId() i .s'
  Pop $FoldSentinelUninstallParentPid

  ClearErrors
  CopyFiles /SILENT "$INSTDIR\${MAINBINARYNAME}.exe" "$FoldSentinelUninstallHelper"
  IfErrors FoldSentinelUninstallPrepareLaunchFailed
  IfFileExists "$FoldSentinelUninstallHelper" 0 FoldSentinelUninstallPrepareLaunchFailed

  ; /UPDATE 是公开的命令行参数，不能作为绕过预检的凭据。
  ClearErrors
  ${If} $FoldSentinelUninstallUnattended == 1
    ExecWait '"$FoldSentinelUninstallHelper" --uninstall-prepare --parent-pid $FoldSentinelUninstallParentPid --silent' $FoldSentinelUninstallExitCode
  ${Else}
    ExecWait '"$FoldSentinelUninstallHelper" --uninstall-prepare --parent-pid $FoldSentinelUninstallParentPid' $FoldSentinelUninstallExitCode
  ${EndIf}
  IfErrors FoldSentinelUninstallPrepareLaunchFailed
  ${If} $FoldSentinelUninstallExitCode == 0
    StrCpy $FoldSentinelUninstallPrepared 1
  ${ElseIf} $FoldSentinelUninstallExitCode == 10
    ; 强制卸载仅允许 helper 已完成交互式免责声明和双重确认的分支。
    ${If} $FoldSentinelUninstallUnattended == 1
      StrCpy $FoldSentinelUninstallExitCode 20
      Goto FoldSentinelUninstallPrepareBlocked
    ${EndIf}
    StrCpy $FoldSentinelUninstallPrepared 1
    StrCpy $FoldSentinelUninstallForced 1
  ${Else}
    Goto FoldSentinelUninstallPrepareBlocked
  ${EndIf}
  Goto FoldSentinelUninstallPrepareComplete

FoldSentinelUninstallPrepareLaunchFailed:
  StrCpy $FoldSentinelUninstallExitCode 21
  DetailPrint "FoldSentinel: could not start the uninstall safety check."
  ${If} $FoldSentinelUninstallUnattended == 0
    ${If} $LANGUAGE == 2052
      MessageBox MB_OK|MB_ICONSTOP "无法启动卸载安全检查，尚未删除程序。请重新安装修复后重试。"
    ${Else}
      MessageBox MB_OK|MB_ICONSTOP "The uninstall safety check could not start. No program files have been removed. Repair the installation and try again."
    ${EndIf}
  ${EndIf}
FoldSentinelUninstallPrepareBlocked:
  DetailPrint "FoldSentinel: uninstall safety check stopped removal (exit $FoldSentinelUninstallExitCode)."
  SetErrorLevel $FoldSentinelUninstallExitCode
  Quit
FoldSentinelUninstallPrepareComplete:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $FoldSentinelUninstallPrepared != 1
    StrCpy $FoldSentinelUninstallExitCode 22
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
  ; 必须先验证程序删除结果，再调用会清理正常卸载恢复资料的 finish。
  ; 路径覆盖仅供隔离夹具使用，产品默认值与生成的 Tauri 卸载段一致。
  !ifndef FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT
    !define FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT "$DESKTOP\${PRODUCTNAME}.lnk"
  !endif
  !ifndef FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT
    !define FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
  !ifndef FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT
    !define FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !endif
  !ifndef FOLDSENTINEL_UNINSTALL_RUN_KEY
    !define FOLDSENTINEL_UNINSTALL_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
  !endif
  !insertmacro FoldSentinelVerifyRemovedFile "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro FoldSentinelVerifyRemovedFile "$INSTDIR\foldsentinel-service.exe"
  !insertmacro FoldSentinelVerifyRemovedFile "$INSTDIR\uninstall.exe"
  !insertmacro FoldSentinelVerifyRemovedShortcut "${FOLDSENTINEL_UNINSTALL_DESKTOP_SHORTCUT}"
  !insertmacro FoldSentinelVerifyRemovedShortcut "${FOLDSENTINEL_UNINSTALL_STARTMENU_SHORTCUT}"
  !insertmacro FoldSentinelVerifyRemovedShortcut "${FOLDSENTINEL_UNINSTALL_STARTMENU_FOLDER_SHORTCUT}"
  !if "${INSTALLMODE}" == "perMachine"
    !insertmacro FoldSentinelVerifyRemovedRegistryKey 0x80000002 "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "both"
    ${If} $MultiUser.InstallMode == "AllUsers"
      !insertmacro FoldSentinelVerifyRemovedRegistryKey 0x80000002 "${UNINSTKEY}"
    ${Else}
      !insertmacro FoldSentinelVerifyRemovedRegistryKey 0x80000001 "${UNINSTKEY}"
    ${EndIf}
  !else
    !insertmacro FoldSentinelVerifyRemovedRegistryKey 0x80000001 "${UNINSTKEY}"
  !endif
  ; 自启动值只属于当前用户，不是恢复资料；无论普通或强制卸载都移除本产品项。
  !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 "${FOLDSENTINEL_UNINSTALL_RUN_KEY}" "${PRODUCTNAME}"
  !insertmacro FoldSentinelVerifyRemovedRegistryValue 0x80000001 "${FOLDSENTINEL_UNINSTALL_RUN_KEY}" "${PRODUCTNAME}"
  ClearErrors
  ${If} $FoldSentinelUninstallUnattended == 1
    ExecWait '"$FoldSentinelUninstallHelper" --uninstall-finish --parent-pid $FoldSentinelUninstallParentPid --silent' $FoldSentinelUninstallExitCode
  ${Else}
    ExecWait '"$FoldSentinelUninstallHelper" --uninstall-finish --parent-pid $FoldSentinelUninstallParentPid' $FoldSentinelUninstallExitCode
  ${EndIf}
  IfErrors FoldSentinelUninstallFinishLaunchFailed
  ${If} $FoldSentinelUninstallExitCode != 0
    Goto FoldSentinelUninstallFinishFailed
  ${EndIf}
  ; 普通与强制卸载都清理已知产品安装登记；恢复 JSON、ProgramData 与
  ; 强制保留的应用用户数据仍由 helper 按原协议处理，不在此删除。
    !if "${INSTALLMODE}" == "perMachine"
      !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000002 "${MANUPRODUCTKEY}" ""
    !else if "${INSTALLMODE}" == "both"
      ${If} $MultiUser.InstallMode == "AllUsers"
        !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000002 "${MANUPRODUCTKEY}" ""
      ${Else}
        !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 "${MANUPRODUCTKEY}" ""
      ${EndIf}
    !else
      !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 "${MANUPRODUCTKEY}" ""
    !endif
    ; Tauri 的安装器语言始终保存在当前账户，即使程序按机器安装。
    !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
  Goto FoldSentinelUninstallFinishComplete

FoldSentinelUninstallFinishLaunchFailed:
  StrCpy $FoldSentinelUninstallExitCode 23
FoldSentinelUninstallFinishFailed:
  DetailPrint "FoldSentinel: uninstall cleanup did not complete (exit $FoldSentinelUninstallExitCode)."
  ${If} $FoldSentinelUninstallUnattended == 0
    ${If} $LANGUAGE == 2052
      MessageBox MB_OK|MB_ICONSTOP "卸载收尾未完成，不能确认完整卸载。程序文件、系统入口或应用数据可能仍有残留，恢复资料不会因程序删除失败而清理。请根据日志处理。"
    ${Else}
      MessageBox MB_OK|MB_ICONSTOP "Uninstall cleanup did not complete. Program files, system entries or app data may remain. Recovery records are retained when program removal fails. Review the logs before retrying."
    ${EndIf}
  ${EndIf}
  SetErrorLevel $FoldSentinelUninstallExitCode
  Quit
FoldSentinelUninstallFinishComplete:
!macroend

!endif
