# RTL8821CU USB 隔空投送

Wayline 的 `rtl8821cu` 后端使用专用 USB 网卡运行 AWDL。桌面和 CLI 继续使用 Wayline 的设备发现、收发、确认及传输历史。普通 Wi-Fi 由另一张网卡连接。

| 方案 | 对应硬件和用途 |
| --- | --- |
| [DropBridge](https://github.com/ndpyzwy-0w0/DropBridge) | 作者已用 RTL8821CU USB `0BDA:C820` 验证 iPhone → Windows 接收。Wayline 借用其独立无线进程，完整收发仍需实测。 |
| [WinAWDL](https://github.com/raph559/winawdl) | 有板载 RTL8852CE `10EC:C852` 的双向实测。开启无线功能时接管板载网卡。 |
| [NekoDrop](https://github.com/zheznanohana/nekodrop) | 面向特定 Intel PCIe 网卡，支持同卡上网与 AWDL。Windows 11 顶层菜单通过稀疏包声明和 `IExplorerCommand` 实现。Wayline 目前在经典右键菜单和“发送到”中提供入口。 |
| [Wayline 的 ESP32 后端](QUICKSTART.zh-CN.md) | 通过串口使用 ESP32-S3/C5。USB 无线后端可以在相同桌面和 CLI 中选用。 |

DropBridge 的发送实验已验证可通过 USB 无线后端连接 iPhone，但实验栈提供的自签 TLS 客户端证书被拒绝。Wayline 使用 Owly 发起传输，在发送端沿用 Owly 与 NekoDrop 的无客户端证书配置。参考 [DropBridge 发送实验](https://github.com/ndpyzwy-0w0/DropBridge/blob/v0.1.0-internal.1/docs/SEND_PATH.md)和 [NekoDrop #13](https://github.com/zheznanohana/nekodrop/pull/13)。

Windows 版 omdrop、SwooshDrop 和 HowlDrop 也提供了资料。前者用 ESP32 并将链接交给桌面操作，SwooshDrop 的硬件 beta 需要 WSL2，HowlDrop 的原生路线有自己的 MT7612U 用户态驱动。Wayline 这里复用的是 DropBridge 已发布的 RTL8821CU 无线程序和本仓库已有的 Owly。参考 [omdrop-owl-windows](https://github.com/dantdmdarealguy/omdrop-owl-windows)、[SwooshDrop](https://github.com/AlbinoPriest/swooshdrop)、[HowlDrop](https://github.com/MrSco/howl-drop/blob/main/docs/native-windows.md)。

## 准备

1. Windows 应枚举到一个 `USB\VID_0BDA&PID_C820&MI_02`。这是 Wi-Fi 接口。电脑的蓝牙用于 AirDrop 发现。
2. 在管理员权限的 [Zadig](https://zadig.akeo.ie/) 中打开 **Options → List All Devices**，核对 USB ID 为 **0BDA C820、Interface 2 (MI_02)**，将这一接口的驱动替换为 **WinUSB**。详见 [DropBridge 绑定与回退说明](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/docs/BINDING_RUNBOOK.md)。
3. 构建 Windows 主机程序。在 `wayline-bridge.exe` 旁放置校验过的 `wintun.dll`，并在同目录的 `usb-radio\` 安放 [DropBridge v0.1.0-internal.1](https://github.com/ndpyzwy-0w0/DropBridge/releases/tag/v0.1.0-internal.1) 包内 `backend\airbridge_radio.exe` 和 `backend\libusb-1.0.dll`。无线程序的 SHA-256 应为 `3805ec83369efdcfadbdc1a2680cda41efaecff76b0fa0ae522ebe27a7641b9f`。无线程序需要 [Microsoft Visual C++ v14 x64 运行库](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist#latest-supported-redistributable-version)，许可证见 DropBridge 原包。
4. 停止 Wayline 后，在 `%ProgramData%\wayline\config.json` 中设置以下字段，保留已有的其他设置，再用项目的服务安装入口注册此构建。服务目录应位于仅管理员可写的目录。前台诊断可在管理员终端运行 `wayline-bridge.exe --rtl8821cu --channel 149`，用 Ctrl+C 停止。

```json
{
  "radioBackend": "rtl8821cu",
  "port": "auto",
  "channel": 149,
  "enabled": true
}
```

`radioBackend` 缺省为 `esp32`。RTL8821CU 使用 20 MHz 固定信道，可选择 6、44 或 149。该无线程序首次初始化可能需要一分多钟。服务日志位于 `%ProgramData%\wayline\bridge-console.log`。退出 DropBridge 托盘程序后再启动 Wayline，同一时间只运行一套无线后端。

## 验收和恢复

1. 在 Wayline 中确认无线设置显示 **RTL8821CU (USB)**、AWDL 就绪，并确认板载网卡的上网链路正常。
2. 用 iPhone 原生分享菜单向 Wayline 传一份小文件。再用 Safari 或 B 站 App 的系统分享菜单投送一个网页链接。Wayline 显示发送者及网址，点“接受并打开链接”后自动在默认浏览器打开。浏览器可以跟随 `b23.tv` 短链接重定向。该入口接收 HTTP(S) 网页地址。
3. 单独验证 Windows → iPhone 文件发送。DropBridge 的接收记录、Wayline 的本机环回测试和此 USB 后端的双向空口验证是不同的结果。
4. 需要恢复 USB 网卡的普通 Wi-Fi 时，先退出 Wayline 并停服务。在设备管理器对 **同一个 `MI_02`** 选择“回退驱动程序”或“更新驱动程序 → 浏览我的电脑 → 让我从可用驱动程序列表中选取”，选回 `Realtek 8821CU Wireless LAN 802.11ac USB NIC`（系统自带的 `netrtwlanu.inf`）。确认它重新出现在 WLAN 列表。恢复 ESP32 模式可将配置中的 `radioBackend` 设回 `esp32` 并按板型设置端口和信道。
