# RTL8821CU USB 隔空投送

Wayline 的 `rtl8821cu` 后端使用专用 USB 网卡运行 AWDL。桌面和 CLI 继续使用 Wayline 的设备发现、收发、确认及传输历史。普通 Wi-Fi 由另一张网卡连接。

| 方案 | 对应硬件和用途 |
| --- | --- |
| [DropBridge](https://github.com/ndpyzwy-0w0/DropBridge) | 作者已用 RTL8821CU USB `0BDA:C820` 验证 iPhone → Windows 接收。Wayline 借用其独立无线进程，并在该硬件组合上使用自己的 AirDrop 收发实现。 |
| [WinAWDL](https://github.com/raph559/winawdl) | 有板载 RTL8852CE `10EC:C852` 的双向实测。开启无线功能时接管板载网卡。 |
| [NekoDrop](https://github.com/zheznanohana/nekodrop) | 面向特定 Intel PCIe 网卡，支持同卡上网与 AWDL。Windows 11 顶层菜单通过稀疏包声明和 `IExplorerCommand` 实现。Wayline 提供单独的[一级菜单扩展](EXPLORER.zh-CN.md)，并保留经典菜单和“发送到”。 |
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

在设置中选好信道和监听带宽，再点「保存」。两个值会一起应用。已就绪的 RTL8821CU 在 20 MHz 下从 44 换到 149 或从 149 换到 44，可以复用当前 USB 无线进程。Wayline 等待无线进程确认新信道，并重新建立 AWDL 接口。无线进程未确认时会按新信道重新初始化。带宽变化需要重新初始化一次。切换前请结束当前收发。

AWDL 用信道 6（2.4 GHz）、44 和 149（5 GHz）进行发现和同步。Apple 设备可按地区及 Wi-Fi 状态调整监听时隙，也可以在双方有共同可用时隙的其他信道上传数据。目前 RTL8821CU 固定监听所选信道。若手机在 44 上一直未出现，可切回 149。附近 Wi-Fi 扫描只能判断接入点和部分接入点报告的信道占用，发现是否可用要以对端响应为准。[AWDL 信道和同步机制](https://arxiv.org/abs/1808.03156)

在桌面设置中可开启「登录后后台启动 Wayline」。Windows 登录后，Wayline 会在托盘启动服务和接收端。稍后打开分享窗口可复用已初始化的无线进程；从托盘退出仍会按原方式关闭按需服务。

5 GHz 表示无线频段。这个后端目前使用 20 MHz 信道和 12 Mbit/s 的基本发帧速率，实际传输还受空口调度、确认、重传和 USB 写入速度影响。当前组合在信道 149 已完成双向文件传输。RTL8821CU 在 −75 dBm 以上接纳新设备的广播，并在桥接日志记录已接纳设备的信号强度。测速度时可保持两端相邻、使用同一文件和信道，并记录传输历史里的实际字节和耗时。

## 验收和恢复

1. 在 Wayline 中确认无线设置显示 **RTL8821CU (USB)**、AWDL 就绪，并确认板载网卡的上网链路正常。
2. 用 iPhone 原生分享菜单向 Wayline 传一份小文件。再用 Safari 或 B 站 App 的系统分享菜单投送一个网页链接。Wayline 显示发送者及网址，点“接受并打开链接”后自动在默认浏览器打开。浏览器可以跟随 `b23.tv` 短链接重定向。该入口接收 HTTP(S) 网页地址。
3. 保持 iPhone 解锁并退出手机的系统分享面板，在 Wayline 选择一个小文件，打开设备列表，验证 Windows → iPhone 文件发送。
4. 需要恢复 USB 网卡的普通 Wi-Fi 时，先退出 Wayline 并停服务。在设备管理器对 **同一个 `MI_02`** 选择“回退驱动程序”或“更新驱动程序 → 浏览我的电脑 → 让我从可用驱动程序列表中选取”，选回 `Realtek 8821CU Wireless LAN 802.11ac USB NIC`（系统自带的 `netrtwlanu.inf`）。确认它重新出现在 WLAN 列表。恢复 ESP32 模式可将配置中的 `radioBackend` 设回 `esp32` 并按板型设置端口和信道。

## 吞吐对照

无线设置显示 USB 启动阶段和耗时。百分比按桥接启动、网卡初始化、校验助手、准备 AWDL 接口、服务就绪这几个检查点推进。USB 助手初始化花多久由设备决定。只有服务确认当前桥接实例的 AWDL 接口上线，进度才到 100%。

Wayline 的 RTL8821CU AirDrop 后端默认使用 20 MHz。设置 → 无线频段与信道 → 监听带宽可选择 20、40 或 80 MHz。40/80 MHz 是 5 GHz 信道 44、149 的实验档，切换会重启 USB 无线后端。启动明确失败时，服务会尝试恢复 20 MHz。下次回到信道 6 前需先选择 20 MHz。该选项设置监听宽度，发送仍使用配置的 Legacy 或 HT20 速率。RTL8821CU 的 80 MHz 物理层标称上限为 433.3 Mbit/s，160 MHz 超出硬件规格。[RTL8821CU 模组规格书](https://fccid.io/m/274b8325ef6cf030bd750065fe550302dc7bc61764fd653d060b45ccd53245c8.pdf)

DropBridge 用同款芯片在 40 MHz 完成过传输，后续长期测试出现降速和停顿，因此默认退回 20 MHz。80 MHz 在校正中心信道后仍未通过 iPhone 发现。搜不到设备或传输失败时可切回 20 MHz。同一轮吞吐对照只改变带宽、速率、定时或 MTU 其中一项。[40 MHz 测试](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/docs/STATUS.md#4-关键配置由监管程序固定) · [80 MHz 实机记录](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/evidence/airdrop-userspace-width80-attempt.txt)

这些选项只适用于 USB RTL8821CU。结束收发、退出桌面和其他 Wayline 客户端后，确认 Windows 的 `Wayline` 服务已停止。如果服务仍以常驻模式运行，在管理员 PowerShell 中执行 `Stop-Service Wayline`。备份并更新 `%ProgramData%\wayline\config.json`，再重新打开 Wayline。下面是可并入现有配置的基准值。

```json
{
  "rtl8821cuWidthMhz": 20,
  "rtl8821cuRateProfile": "baseline",
  "rtl8821cuHighResolutionTimer": false,
  "rtl8821cuMtu": 1450
}
```

在信道 149、同一摆放位置和可比的手机 Wi‑Fi 状态下，用同一份大文件分别测发送和接收。每次只改变一项，完成后在历史记录中比较有效传输字节和耗时。

- `rtl8821cuHighResolutionTimer: true` 在无线助手就绪后向 Windows 申请 1 ms 定时分辨率，可用桥接日志的 `timer late` 统计比较调度延迟。关闭 Wayline 后会释放该请求。
- `rtl8821cuRateProfile: "ht20-mcs4"` 或 `"ht20-mcs7"` 只向声明支持所选速率的对端发送 HT20 数据帧。广播和对端未声明支持时使用 12 Mbit/s。桥接日志的 `queued unicast data rates` 区分已入队的 Legacy 和 HT 帧。
- `rtl8821cuMtu: 1440` 降低 Windows AWDL 接口的出站 MTU。对当前 pinned 无线助手，一个 1450 字节 IPv6 包加上无线头及 USB TX 描述符是 1538 字节。1440 字节的选项可避开这类出站帧跨越 1536 字节边界的情况。比较日志的 `bulk_send` 部分超时和文件完成率。

还原基准值并重启可回退。切回 ESP32 时先还原这些 USB 专用设置。
