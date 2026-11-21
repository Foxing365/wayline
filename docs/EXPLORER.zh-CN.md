# Windows 11 一级右键菜单

Wayline 桌面端会为当前用户注册经典菜单和「发送到」。文件和文件夹都可以通过这些入口进入同一个发送界面。Windows 11 一级菜单需要额外安装已签名的 Wayline Explorer 稀疏包。

## 安装和卸载

1. 把 `wayline-explorer.dll` 和 `Assets\Wayline.png` 放到 `wayline-desktop.exe` 所在的安装目录，保留 `Assets` 子目录。使用与 Windows 资源管理器架构匹配的版本。目前提供 x64。
2. 用日常登录的用户运行以下命令。对于正式分发，请使用受该电脑信任的代码签名证书签署的包。

```powershell
powershell -NoProfile -File .\apps\explorer-menu\register.ps1 `
  -PackagePath .\dist\explorer-menu\Wayline.ExplorerMenu.msix `
  -InstallDirectory 'C:\Program Files\Wayline'
```

在资源管理器中，选中一个或多个文件，或选中文件夹，右击并选择「通过 Wayline 发送」。命令把选中的文件和目录一次性传给已安装的 Wayline。选择中包含无法转换成文件系统路径的虚拟项目时，请从实际文件夹发起传输。首次注册或更新后如果仍然只看到旧菜单，可重新启动资源管理器或注销并重新登录。

```powershell
powershell -NoProfile -File .\apps\explorer-menu\register.ps1 -Uninstall
```

卸载会移除此用户的一级菜单包。经典入口可用 `wayline-desktop.exe --unregister-shell` 移除。

## 构建和签名

在 x64 Visual Studio Developer PowerShell 中运行以下命令。`SdkBin` 为含 `MakeAppx.exe` 和 `SignTool.exe` 的 Microsoft Windows SDK 目录。`SigningThumbprint` 为当前用户个人证书存储中用于代码签名的证书指纹。自定义签名证书的主题用于填充 MSIX 的 `Publisher`。

```powershell
.\apps\explorer-menu\build.ps1 -OutputDirectory .\dist\explorer-menu `
  -SdkBin 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64' `
  -PackageVersion '1.0.0.0' -SigningThumbprint $env:WAYLINE_SIGNING_THUMBPRINT
```

构建产生 `wayline-explorer.dll`、`Assets\Wayline.png` 和 `Wayline.ExplorerMenu.msix`。也可用 `-Compiler` 指定 x64 `clang-cl.exe` 或 LLVM MinGW 的 `x86_64-w64-mingw32-clang++.exe`。只验证编译及封包时可使用 `-Unsigned`，无需签名证书。

发布新版时递增 `PackageVersion`。同一版本的包需要先卸载，再注册。安装脚本使用当前用户的包注册接口，证书信任由系统现有配置决定。开发机如需测试自签证书，可按 Microsoft 的步骤显式导入签名证书的公开部分。分发包时保留私钥在签名端。

右键命令的 `IExplorerCommand` 由系统 COM 代理载入。Windows 可在传入选中项前查询命令是否可用。命令在执行时解析并验证文件路径，再调用 `wayline-desktop.exe --send -- <路径…>`。同一 EXE 的单实例机制接收启动参数，和经典入口共用文件校验及发送流程。参考 [Microsoft 的 Explorer 集成说明](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)和[外部位置包的签名与注册说明](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)。
