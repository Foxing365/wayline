# Wayline

Wayline brings AirDrop file, photo, and folder transfers to Windows. A dedicated RTL8821CU USB adapter handles AirDrop while a separate Wi-Fi adapter stays connected to your usual network.

**[Download the latest Windows x64 release](https://github.com/Foxing365/wayline/releases/latest)** · [简体中文](README.zh-CN.md)

## Get started

Use Windows 10 or 11 x64 with Bluetooth LE advertising. The tested USB Wi-Fi interface has hardware ID `USB\VID_0BDA&PID_C820&MI_02`. Check the ID in Device Manager before setting it up.

1. Download and extract the portable Windows x64 ZIP. Check its SHA-256 against the release page and open the included installation and rollback guide, `安装和回退.md`.
2. Install WinUSB on **only** the USB adapter’s `MI_02` Wi-Fi interface. Obtain the pinned DropBridge radio helper and libusb DLL separately, then put them in the `usb-radio` directory beside the Wayline bridge. The USB guide covers file verification, the required Microsoft Visual C++ x64 runtime, and driver rollback. [RTL8821CU setup and rollback](docs/RTL8821CU.zh-CN.md)
3. Follow the package guide to register the Wayline Windows service as an administrator. Start `wayline-desktop.exe` as your normal user. Wait for **RTL8821CU (USB)** to be ready.
4. Enable Bluetooth on the PC and turn on Wi-Fi and Bluetooth on the iPhone. Set iPhone AirDrop to **Everyone for 10 Minutes**. Send a small file from the iPhone’s Share menu or select a file and recipient in Wayline.

M5Stack Cardputer Adv and compatible ESP32-S3/C5 boards are an alternative radio path. Prepare the matching firmware and flashing tools separately, then select the port and profile under **Settings → Device firmware**. [ESP32 quick start](docs/QUICKSTART.md) · [Firmware packaging](docs/PACKAGING.md)

## Daily use

- **Send from Explorer or Wayline.** Drag and drop, use the classic right-click menu, or choose **Send to → Wayline AirDrop**. An optional Windows 11 first-level menu uses a separately installed, signed extension. [Explorer extension setup](docs/EXPLORER.zh-CN.md)
- **Receive in one window.** The sharing card slides in from the tray without taking focus and shows progress, speed, elapsed time, and **Cancel**. Completion stays in the same card for **10 seconds by default**, adjustable through the 3–30 second options in Settings.
- **Handle files and text.** Preview images and open, save a copy of, copy, or move received files. The toolbar folder button opens the receive directory, which defaults to `Downloads\Wayline`. Text over 200 characters previews the first 200, with the full text available to copy.
- **Choose what to trust.** Accept each offer or trust the sender’s verified certificate for future file offers. Browser opening has its own permission. A TXT share from a sender with that permission opens automatically when it contains fewer than 120 characters and exactly one distinct HTTP(S) URL.
- **Make it yours.** Choose the original Wayline, NekoDrop Light, or NekoDrop Dark layout. Enable launch at Windows sign-in for background receiving. Recent transfers shows the result and details of each transfer.

## Tested and supported

- **Channel 149.** The `0BDA:C820 MI_02` setup completed transfers with an iPhone in both directions at 20 MHz. Wayline received one live batch of **61 photos** at 40 MHz. A separate TLS regression verified an incoming directory containing **4,100 files**. [Regression test](crates/wayline-core/tests/airdrop_migration.rs)
- **40 MHz and 80 MHz.** These 5 GHz monitor widths are experimental. DropBridge observed degraded performance over time at 40 MHz and found no iPhone discovery after correcting the 80 MHz center channel. If discovery or transfers are unreliable, try 20 MHz. [40 MHz results](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/docs/STATUS.md#4-关键配置由监管程序固定) · [80 MHz test](https://github.com/ndpyzwy-0w0/DropBridge/blob/main/evidence/airdrop-userspace-width80-attempt.txt)
- **Contacts Only.** The regular setup uses **Everyone for 10 Minutes**. Sending to a Contacts Only device using a separately obtained AirDrop identity is experimental and needs validation with your devices. [Identity setup](docs/QUICKSTART.md#send-to-a-contacts-only-device)

## Source and credits

[Build setup](docs/SETUP.md) · [Development and CLI](docs/DEVELOPMENT.md) · [Packaging](docs/PACKAGING.md)

Thanks to [Owly](https://github.com/t4t5/owly), [OWL](https://github.com/seemoo-lab/owl), [jedbillyb/owl](https://github.com/jedbillyb/owl), [omdrop-owl-windows](https://github.com/dantdarealguy/omdrop-owl-windows), [omdrop](https://github.com/netmojo-systems/omdrop-plugin), [omdrop-awdl](https://github.com/netmojo-systems/omdrop-awdl), [OpenDrop](https://github.com/seemoo-lab/opendrop), and [Windfall](https://github.com/UvejsGj/Windfall) for the radio and transfer foundations. The USB setup uses the [DropBridge](https://github.com/ndpyzwy-0w0/DropBridge) helper. [WinAWDL](https://github.com/raph559/winawdl) and [NekoDrop](https://github.com/zheznanohana/nekodrop) informed the Windows work.

The desktop uses [GPUI](https://gpui.rs/) and [Wintun](https://www.wintun.net/). The ESP32 builds use [ESP-IDF](https://github.com/espressif/esp-idf), [esptool](https://github.com/espressif/esptool), [espup](https://github.com/esp-rs/espup), [M5Unified](https://github.com/m5stack/M5Unified), [M5GFX](https://github.com/m5stack/M5GFX), and [Noto CJK](https://github.com/notofonts/noto-cjk). [Firmware font license](firmware/assets/OFL.txt)

Wayline is distributed under [GPL-3.0-only](LICENSE). The release ZIP includes dependency licenses and notices.
