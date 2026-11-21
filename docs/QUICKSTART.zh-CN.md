# Wayline 快速开始

[English](QUICKSTART.md)

需要 Windows 10/11 x64 和支持蓝牙 LE 广播的电脑。用专用 RTL8821CU USB 网卡时按[网卡指南](RTL8821CU.zh-CN.md)准备。用 ESP32 时准备兼容开发板、对应固件和 USB 数据线。

## 安装与更新

从 [GitHub Releases](https://github.com/Foxing365/wayline/releases/latest) 下载并解压 Windows 主机包。在解压目录运行 `wayline-tools.exe release-verify .` 校验文件。把主机程序放在同一个由管理员管理的程序目录，例如 `C:\Program Files\Wayline`。准备好无线后端后，打开管理员 PowerShell 注册服务。

```powershell
Set-Location "$env:ProgramFiles\Wayline"
.\wayline-tools.exe service-install --binary (Join-Path $PWD.Path 'wayline-desktop.exe')
```

用普通权限启动 `wayline-desktop.exe`，以便使用资源管理器拖放。更新前先完成传输，从托盘退出 Wayline 并结束 CLI 命令。替换服务程序前，先停止 Wayline 服务。接收文件和设备设置与程序目录分开保存。

## 初始化开发板

使用 ESP32 后端时，先按[打包指南](PACKAGING.zh-CN.md)准备固件和烧录工具。

1. 用数据线连接设备，打开「设置 → 设备固件」。
2. 选择对应串口和固件 profile：Cardputer Adv、ESP32-S3 Headless UART0 / Native USB，或 ESP32-C5 UART0 / Native USB。UART0 与 Native USB 必须匹配实际接入电脑的 USB 接口类型。S3 Headless profile 固定适用于 8 MiB Flash。
3. 首次使用选“首次 / 重新安装”，会替换程序和分区布局，不保证保留其他固件的数据。已有 Wayline 选“保留设置更新”，要求原固件可识别且分区布局一致。
4. 点击“开始安装”或“开始更新”，等待写入、校验和启动检查完成，显示“设备已就绪”。

安装期间保持连接。关闭卡片只隐藏界面，托盘退出会询问是否取消；写入开始后的取消或断电可能需要完整重刷，不支持自动回滚。

## 收发与设置

电脑打开蓝牙；iPhone 打开 Wi-Fi 和蓝牙，将 AirDrop 设为“所有人（10 分钟）”。电脑内置 Wi-Fi 无需开启，也无需连接同一个路由器。

- 发送：拖入文件，或用托盘、`Ctrl+O`、文件右键“通过 Wayline 发送”选取文件，再选择接收设备。上传到 100% 后等待“已送达”。
- 接收：在 Apple 设备的 AirDrop 列表选择这台电脑，再在 Wayline 中接受。
- 设置：可修改设备名称、接收目录和界面语言；语言使用下拉框选择，支持简体中文、繁體中文、English、日本語、한국어、Français、Deutsch 和 Español。名称与目录桌面和 CLI 共用。默认保存到下载目录的 `Wayline` 文件夹；更改目录不移动已有文件，接收期间不可更改。语言选择保存在本机并立即生效。
- 错误详情：鼠标悬停提示可展开全文，点击“复制”获取完整诊断。

### 向「仅限联系人」设备发送

在桌面设置找到「发给『仅限联系人』」，选择包含自己已取得的 AirDrop 身份文件的文件夹。文件名为 `certificate.pem`、`key.pem`、`validation_record.cms`。分别放置证书链、与叶证书配对的未加密 PEM 私钥、Apple 签发的 DER 验证记录。每次运行要使用此身份时，勾选「本次运行使用 Apple 身份查找和发送」。接收方通讯录需包含验证记录对应的 Apple 账户邮箱或手机号。此功能目前为实验功能，实际发现和传输需按设备验证。[Apple 的 AirDrop 身份说明](https://support.apple.com/zh-cn/guide/security/sec2261183f4/web) · [Omdrop 身份文件说明](https://github.com/netmojo-systems/omdrop-plugin#your-airdrop-identity)

Wayline 在当前 Windows 用户下使用 DPAPI 加密保存导入材料。「移除身份」删除应用保存的副本。现有的 AirDrop 身份须由用户提供，Apple 没有公开完整身份的常规导出步骤。Apple 设备显示的已验证联系人照片来自该设备的通讯录。

× 或 Esc 隐藏卡片，接收继续运行。托盘“退出 Wayline”或 `Ctrl+Q` 结束本客户端任务；默认按需服务在最后一个客户端退出后回收无线设备，独立服务部署可以常驻。

首次启动会注册文件右键菜单和“发送到 → Wayline AirDrop”。Windows 11 中通常位于“显示更多选项”。退出后运行 `wayline-desktop.exe --unregister-shell` 可移除入口。

## 板载界面

界面支持简体中文、繁體中文和 English，提供状态、分享指南、偏好设置和更多功能四页。双向传输分别显示状态卡；“已连接”表示近期收到电脑命令，文件结果以传输状态卡为准。

| 按键 | 操作 |
| --- | --- |
| Tab 或 1 / 2 / 3 / 4 | 切换页面 |
| L | 切换语言 |
| B / M | 切换亮度 / 开关动效 |
| A | 打开声音页；在声音页开关任务提醒 |
| V | 切换音色并试听 |
| − / +（或 =） | 调整声音页音量，每次 5% |
| P | 在声音页试听 |
| T | 设置自动熄屏：关闭、30 秒、2 分钟、5 分钟 |
| S | 立即熄屏，任意键唤醒 |
| H / I | 连接帮助 / 设备信息 |
| W | 重播欢迎动画 |
| R（设置或关于页） | 恢复默认设置；Enter 确认，其他键取消 |
| Backspace | 返回上一级或状态页 |

偏好自动保存。屏幕休眠期间无线通信和声音提醒继续运行；新传输唤醒屏幕，活动传输期间暂停自动熄屏。

## 可选命令行工具

CLI 工具包解压后可直接在终端中运行；无线收发需要已启动的桌面或独立后台服务。

```sh
./wayline-cli.exe --help
./wayline-cli.exe status --json
./wayline-cli.exe discover
./wayline-cli.exe send --to "iPhone" -- "C:\Users\Example\Pictures\photo.jpg"
./wayline-cli.exe receive
```

接收默认逐次询问，无人值守接收需明确添加 `--accept-all`。Ctrl+C 结束命令并释放自己的会话。完整参数、JSON 输出与退出码见[开发指南](DEVELOPMENT.zh-CN.md#cli-与串口诊断)。

## 常见问题

- 未发现设备：检查数据线并刷新端口；必要时按住 G0，再点按 RST 进入下载模式。
- 写入后启动失败：松开 G0，再点按 RST，保持连接后重试。
- 端口被占用：结束其他 CLI 传输、串口监视器或烧录软件。
- 无法拖放：以普通权限启动桌面。
- 换端口重刷后仍未恢复：新端口成功不会清除旧端口的失败记录，见[维护恢复](DEVELOPMENT.zh-CN.md#服务配置与-ipc)。

[返回项目说明](../README.zh-CN.md)
