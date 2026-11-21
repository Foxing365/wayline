# Wayline quick start

[简体中文](QUICKSTART.zh-CN.md)

Use Windows 10/11 x64 and a PC with Bluetooth LE advertising. To use the dedicated RTL8821CU USB adapter, follow the [adapter setup guide](RTL8821CU.zh-CN.md). To use an ESP32 backend, prepare the supported board, firmware package, and a USB data cable.

## Install and update

Download the [Windows host package](https://github.com/Foxing365/wayline/releases/latest).
Extract it and run `wayline-tools.exe release-verify .` from the extracted directory.
Move the host files together into an administrator-managed folder such as `C:\Program Files\Wayline`.
Configure the radio backend, then register the Windows service in an elevated PowerShell window.

```powershell
Set-Location "$env:ProgramFiles\Wayline"
.\wayline-tools.exe service-install --binary (Join-Path $PWD.Path 'wayline-desktop.exe')
```

Start `wayline-desktop.exe` as a normal user so Explorer drag-and-drop works.

Before updating program files, finish transfers, quit Wayline from the tray, and close CLI commands.
Stop the Wayline service before replacing a running service binary. Device and receive settings are stored separately from the program files.

## Initialize the Board

For the ESP32 backend, prepare the board image and flashing tools as described in the [packaging guide](PACKAGING.md).

1. Connect the board with a data cable and open Device firmware in Settings.
2. Select the serial port and matching profile: Cardputer Adv, ESP32-S3 Headless UART0 / Native USB, or ESP32-C5 UART0 / Native USB. UART0 and Native USB profiles must match the USB interface connected to the PC. S3 Headless profiles require 8 MiB flash.
3. For first use, choose First install / Reinstall. This replaces the program and partition
   layout and may erase data from other firmware. For Wayline updates, choose Preserve settings;
   this requires recognizable firmware and an identical partition layout.
4. Start installation or update. Wait for writing, verification and the boot check to finish
   and for the device to report ready.

Keep the board connected. Closing the card only hides it; tray Quit asks whether to cancel.
Cancellation or power loss after writing begins may require a complete reflash; automatic
rollback is unavailable.

## Transfers and settings

Enable Bluetooth on the PC. On iPhone, enable Wi-Fi and Bluetooth and set AirDrop to Everyone
for 10 Minutes. The PC's built-in Wi-Fi can stay off; no shared router connection is needed.

- Send: drop files, or select them from the tray, with `Ctrl+O`, or through Explorer's
  Send with Wayline menu. Select a recipient and wait for Delivered after uploading reaches 100%.
- Receive: select this PC in the Apple device's AirDrop list, then accept in Wayline.
- Settings: change the device name, receive folder, or interface language. The language dropdown
  offers Simplified Chinese, Traditional Chinese, English, Japanese, Korean, French, German and
  Spanish. The name and folder are shared by desktop and CLI. The receive folder defaults to
  Downloads/Wayline; changing it leaves existing files in place and is blocked while receiving.
  The language choice is saved locally and applies immediately.
- Error details: hover over a notice to expand it, then use Copy to get complete diagnostics.

### Send to a Contacts Only device

Open **Send to Contacts Only** in desktop settings and choose a folder containing your AirDrop
identity. Put the certificate chain in `certificate.pem`, its matching unencrypted PEM private key in `key.pem`,
and the Apple-signed DER validation record in `validation_record.cms`. Enable the identity for
discovery and sending each time you want to use it. The recipient's Contacts app needs the email or
phone number associated with the record. Support is experimental and requires verification on your
devices. [Apple AirDrop identity](https://support.apple.com/en-ie/guide/security/sec2261183f4/web) ·
[Omdrop identity files](https://github.com/netmojo-systems/omdrop-plugin#your-airdrop-identity)

Wayline uses DPAPI to encrypt the imported identity for your Windows user. **Remove identity**
deletes the copy stored by Wayline. Imports require an existing AirDrop identity. Apple has no
documented standard export for the complete identity. Apple devices use their own contact photos
for verified peers.

× or Esc hides the card while receiving continues. Tray Quit or `Ctrl+Q` ends this client's
tasks. The default on-demand service reclaims the radio after the last client exits;
a standalone service deployment can remain resident.

First launch registers Explorer and Send to → Wayline AirDrop entries. On Windows 11 these
usually appear under Show more options. After quitting, run
`wayline-desktop.exe --unregister-shell` to remove them.

## Board interface

The board supports Simplified Chinese, Traditional Chinese and English, with Status, Sharing
guide, Preferences and More pages. Simultaneous transfers have separate cards.
Connected means a recent host command was received; use transfer cards for file results.

| Key | Action |
| --- | --- |
| Tab or 1 / 2 / 3 / 4 | Switch pages |
| L | Change language |
| B / M | Change brightness / toggle motion |
| A | Open sound settings; toggle task alerts on that page |
| V | Change and preview sound |
| − / + (or =) | Adjust sound-page volume by 5% |
| P | Preview sound on the sound page |
| T | Set display timeout: off, 30 seconds, 2 minutes or 5 minutes |
| S | Sleep the display; any key wakes it |
| H / I | Connection help / device information |
| W | Replay the welcome animation |
| R (Settings or About) | Restore defaults; Enter confirms, any other key cancels |
| Backspace | Return to the previous level or Status |

Preferences save automatically. Wireless operation and sound alerts continue during display
sleep. New transfers wake the display; active transfers suspend automatic sleep.

## Optional CLI toolkit

Extract the CLI toolkit and run it from a terminal. Wireless transfers require an installed
desktop or standalone service package. Start the standalone service binary directly while the
installer migration is in progress.

```sh
./wayline-cli.exe --help
./wayline-cli.exe status --json
./wayline-cli.exe discover
./wayline-cli.exe send --to "iPhone" -- "C:\Users\Example\Pictures\photo.jpg"
./wayline-cli.exe receive
```

Receiving asks for each request by default; unattended receiving requires explicit
`--accept-all`. Ctrl+C ends the command and releases its session. See the
[development guide](DEVELOPMENT.md#cli-and-radio-diagnostics) for arguments, JSON and exit codes.

## Troubleshooting

- No device found: check the data cable and refresh ports. If needed, hold G0 and press RST
  to enter download mode.
- Boot failure after writing: release G0, press RST, keep the board connected and retry.
- Port busy: close other CLI transfers, serial monitors or flashing software.
- Drag-and-drop unavailable: launch desktop without elevation.
- Still blocked after reflashing on a new port: success on the new port does not clear the
  old port's failure record; see [maintenance recovery](DEVELOPMENT.md#service-configuration-and-ipc).

[Back to project overview](../README.md)
