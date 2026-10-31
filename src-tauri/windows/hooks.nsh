!ifndef FOLDSENTINEL_UNINSTALL_HOOKS
!define FOLDSENTINEL_UNINSTALL_HOOKS

!include "LogicLib.nsh"
!include "Win\COM.nsh"
!define /ifndef SLGP_RAWPATH 0x4

Var FoldSentinelUninstallHelper
Var FoldSentinelUninstallParentPid
Var FoldSentinelUninstallExitCode
Var FoldSentinelUninstallUnattended
Var FoldSentinelUninstallPrepared
Var FoldSentinelUninstallForced
Var FoldSentinelUninstallRegistryCode

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
!macro FoldSentinelDeleteKnownRegistryValue ROOT VALUE
  ; System 插件随 NSIS 运行在32位进程中，显式匹配 Tauri 的目标注册表视图。
  !if "${ARCH}" == "x64"
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${MANUPRODUCTKEY}", i 0, i 0x103, *p .r0) i .s'
  !else
    System::Call 'advapi32::RegOpenKeyExW(p ${ROOT}, w "${MANUPRODUCTKEY}", i 0, i 3, *p .r0) i .s'
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
  ${If} $FoldSentinelUninstallForced == 0
    !if "${INSTALLMODE}" == "perMachine"
      !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000002 ""
    !else if "${INSTALLMODE}" == "both"
      ${If} $MultiUser.InstallMode == "AllUsers"
        !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000002 ""
      ${Else}
        !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 ""
      ${EndIf}
    !else
      !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 ""
    !endif
    ; Tauri 的安装器语言始终保存在当前账户，即使程序按机器安装。
    !insertmacro FoldSentinelDeleteKnownRegistryValue 0x80000001 "Installer Language"
    DeleteRegKey /ifempty SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
  ${EndIf}
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
