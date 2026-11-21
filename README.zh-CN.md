# Wayline

Wayline 让 Windows 通过隔空投送与 Apple 设备互传文件、照片和文件夹。专用 RTL8821CU USB 网卡负责隔空投送，电脑的另一张网卡可继续连接平时使用的 Wi‑Fi。

**[下载 Windows x64 最新正式版](https://github.com/Foxing365/wayline/releases/latest)** · [English](README.md)

## 开始使用

电脑需要 Windows 10 或 11 x64，并支持蓝牙 LE 广播。在设备管理器核对专用 USB 网卡的 Wi‑Fi 接口，已实测的硬件 ID 为 `USB\VID_0BDA&PID_C820&MI_02`。

1. 下载并解压 Windows x64 ZIP。对照发布页核对 SHA-256，打开包内的《安装和回退.md》。
2. 只给这张 USB 网卡的 **`MI_02` Wi‑Fi 接口**安装 WinUSB。单独下载指定版本的 DropBridge 无线助手和 libusb DLL，放到 Wayline 桥接程序旁的 `usb-radio` 目录。USB 指南包括校验、Microsoft Visual C++ x64 运行库和驱动回退步骤。[RTL8821CU 配置与回退](docs/RTL8821CU.zh-CN.md)
3. 按包内说明，以管理员权限注册 Wayline 服务。平时用普通权限打开 `wayline-desktop.exe`，等 **RTL8821CU (USB)** 就绪。
4. 电脑打开蓝牙，iPhone 打开 Wi‑Fi 和蓝牙，并把隔空投送设为「所有人（10 分钟）」。从 iPhone 分享菜单试传小文件，也可在 Wayline 中选文件和接收设备反向发送。

也可以用 M5Stack Cardputer Adv 或兼容的 ESP32-S3/C5 开发板。单独准备对应的固件和烧录工具，在「设置 → 设备固件」里选择串口和板型。[ESP32 快速开始](docs/QUICKSTART.zh-CN.md) · [固件打包说明](docs/PACKAGING.zh-CN.md)

## 日常使用

- **从资源管理器或 Wayline 发送。** 支持拖放、经典右键菜单和「发送到 → Wayline AirDrop」。Windows 11 一级右键可通过独立签名扩展安装。[一级右键安装说明](docs/EXPLORER.zh-CN.md)
- **在同一个窗口接收。** 分享窗口从托盘滑入，保留当前应用的焦点。可以查看进度、速度和耗时，或在文件信息旁点「取消」。完成预览默认保留 **10 秒**，设置里可选 3–30 秒。
- **处理文件和分享文本。** 接收后可预览图片，打开、另存、复制或移动文件。顶部文件夹按钮打开接收目录，默认是 `Downloads\Wayline`。文本超过 200 字符时先显示前 200 个，仍可复制原文。
- **自己决定信任谁。** 默认逐次确认，也可信任发送方已验证的证书，之后自动接收该证书发来的文件。自动打开网页需另外授权。经授权设备发送的 TXT 少于 120 字符，且只识别出一个 HTTP(S) 网址时，会自动打开该网页。
- **调整布局和后台接收。** 可选择 Wayline 原版、NekoDrop 浅色或深色。启用开机自启后，Windows 登录即可在后台接收。「最近传输」可查看结果和传输详情。

## 实测与兼容性

- **信道 149。** `0BDA:C820 MI_02` 在 20 MHz 下与 iPhone 完成过双向传输，Wayline 在 40 MHz 下实测接收 **61 张照片**。另有 TLS 回归测试核对了含 **4,100 个文件**的目录。[回归测试](crates/wayline-core/tests/airdrop_migration.rs)
- **40 MHz 和 80 MHz。** 两档是 5 GHz 实验监听带宽。DropBridge 测过 40 MHz 长时间运行后降速，80 MHz 校正中心信道后 iPhone 仍未发现设备。遇到发现或传输问题时，可从 20 MHz 开始排查。[40 MHz 记录](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/docs/STATUS.md#4-关键配置由监管程序固定) · [80 MHz 记录](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/evidence/airdrop-userspace-width80-attempt.txt)
- **仅限联系人。** 常规收发使用「所有人（10 分钟）」。导入已有的隔空投送身份后向「仅限联系人」设备发送仍属实验功能，需要在自己的设备上验证。[身份设置](docs/QUICKSTART.zh-CN.md#向仅限联系人设备发送)

## 源码与致谢

[构建环境](docs/SETUP.zh-CN.md) · [开发与命令行](docs/DEVELOPMENT.zh-CN.md) · [打包说明](docs/PACKAGING.zh-CN.md)

感谢 [Owly](https://github.com/t4t5/owly)、[OWL](https://github.com/seemoo-lab/owl)、[jedbillyb/owl](https://github.com/jedbillyb/owl)、[omdrop-owl-windows](https://github.com/dantdarealguy/omdrop-owl-windows)、[omdrop](https://github.com/netmojo-systems/omdrop-plugin)、[omdrop-awdl](https://github.com/netmojo-systems/omdrop-awdl)、[OpenDrop](https://github.com/seemoo-lab/opendrop) 和 [Windfall](https://github.com/UvejsGj/Windfall) 提供无线与传输基础。USB 方案使用 [DropBridge](https://github.com/ndpyzwy-0w0/DropBridge) 无线助手，Windows 实现参考了 [WinAWDL](https://github.com/raph559/winawdl) 与 [NekoDrop](https://github.com/zheznanohana/nekodrop)。

桌面使用 [GPUI](https://gpui.rs/) 与 [Wintun](https://www.wintun.net/)。ESP32 构建依赖 [ESP-IDF](https://github.com/espressif/esp-idf)、[esptool](https://github.com/espressif/esptool)、[espup](https://github.com/esp-rs/espup)、[M5Unified](https://github.com/m5stack/M5Unified)、[M5GFX](https://github.com/m5stack/M5GFX) 与 [Noto CJK](https://github.com/notofonts/noto-cjk)。[固件字体许可](firmware/assets/OFL.txt)

Wayline 使用 [GPL-3.0-only](LICENSE) 许可证。发布 ZIP 附依赖许可证及声明。
