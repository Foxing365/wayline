use super::{publish_radio_target, wait_for_stop, StopEvent};
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, sync::watch};
use wayline_awdl::{driver, radio_link, windows::wintun::Wintun, MacAddr};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER},
    NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_ALL},
    Networking::WinSock::AF_INET,
    Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
    System::Threading::{CreateEventW, SetEvent, CREATE_NO_WINDOW},
};

use wayline_service::config::{RTL8821CU_HELPER_CLEANUP, RTL8821CU_STARTUP};

pub(super) fn run(
    channel: u8,
    managed: bool,
    helper: PathBuf,
    dll: PathBuf,
    stop: Option<StopEvent>,
) -> io::Result<()> {
    let _guard = if managed {
        None
    } else {
        Some(wayline_service::device_lock::DeviceGuard::try_acquire_rtl8821cu()?)
    };
    let mac = session_mac()?;
    let config = wayline_awdl::configuration(mac.0, "Wayline", "rtl8821cu", channel)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut helper = Helper::start(&helper, channel)?;
            let stopping = wait_for_stop(&stop);
            tokio::pin!(stopping);
            let socket = tokio::select! {
                result = helper.connect(channel, mac) => result?,
                result = &mut stopping => return result,
            };
            let (peers, _) = watch::channel(Vec::new());
            let running = driver::spawn(
                config,
                move || {
                    let net = Wintun::open(&dll, mac)?;
                    Ok((radio_link::Radio::from_socket(socket)?, net))
                },
                peers,
            )?;
            if let Some(path) = std::env::var_os("WAYLINE_BRIDGE_TARGET_FILE") {
                publish_radio_target(Path::new(&path), "rtl8821cu")?;
            }
            tracing::info!("RTL8821CU AWDL ready on channel {channel}");
            let mut failed = running.failed();
            let result = loop {
                if let Some(error) = failed.borrow().as_ref() {
                    break Err(io::Error::other(error.clone()));
                }
                tokio::select! {
                    result = &mut stopping => break result,
                    result = failed.changed() => {
                        if result.is_err() {
                            break Err(io::Error::other("RTL8821CU AWDL worker ended"));
                        }
                    }
                }
            };
            helper.signal_stop();
            running.stop();
            result
        })
}

fn session_mac() -> io::Result<MacAddr> {
    let mut bytes = [0; 6];
    // SAFETY: with the system RNG flag the algorithm handle is null, and the
    // writable output buffer is exactly the length passed to BCrypt.
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            6,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!("system RNG failed: {status:#x}")));
    }
    bytes[0] = (bytes[0] & 0xfc) | 0x02;
    Ok(MacAddr(bytes))
}

struct Helper {
    child: Child,
    stop: OwnedHandle,
    address: SocketAddrV4,
    bringup_until: Option<Instant>,
}

impl Helper {
    fn start(executable: &Path, channel: u8) -> io::Result<Self> {
        let folder = executable.parent().expect("helper path has a parent");
        for path in [executable.to_path_buf(), folder.join("libusb-1.0.dll")] {
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{} missing; provision the DropBridge radio helper beside Wayline",
                        path.display()
                    ),
                ));
            }
        }
        let event: Vec<u16> = "Local\\DropBridgeRadioStop\0".encode_utf16().collect();
        // SAFETY: the event name is terminated and the result is owned once.
        let (handle, already_exists) = unsafe {
            let handle = CreateEventW(ptr::null(), 1, 0, event.as_ptr());
            (handle, GetLastError() == ERROR_ALREADY_EXISTS)
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let stop = unsafe { OwnedHandle::from_raw_handle(handle) };
        if already_exists {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "DropBridge or another radio session is running; exit it before starting Wayline",
            ));
        }

        // Select a different loopback port each launch. Verify the listening
        // process after connecting because another process may win the bind.
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        let mut command = Command::new(executable);
        // Ignore inherited tuning overrides for this fixed hardware profile.
        for (key, _) in std::env::vars_os() {
            if ["AIRBRIDGE_", "DEVOURER_"].iter().any(|prefix| {
                key.to_string_lossy()
                    .to_ascii_uppercase()
                    .starts_with(prefix)
            }) {
                command.env_remove(key);
            }
        }
        let child = command
            .current_dir(folder)
            .args([channel.to_string(), "20".into(), port.to_string()])
            // radio_link's helper parses these with strtoul base 0.
            .env("AIRBRIDGE_VID", "0x0bda")
            .env("AIRBRIDGE_PID", "0xc820")
            .env("AIRBRIDGE_LOG_LEVEL", "warn")
            .env("AIRBRIDGE_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        Ok(Self {
            child,
            stop,
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            bringup_until: Some(Instant::now() + RTL8821CU_STARTUP),
        })
    }

    async fn connect(&mut self, channel: u8, mac: MacAddr) -> io::Result<std::net::TcpStream> {
        let deadline = self.bringup_until.expect("first helper connection");
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(io::Error::other(format!(
                    "RTL8821CU helper exited ({status}); check WinUSB on USB 0BDA:C820 MI_02 and the bridge log"
                )));
            }
            if let Ok(mut socket) = TcpStream::connect(self.address).await {
                if !owns_connection(
                    self.child.id(),
                    self.address.port(),
                    socket.local_addr()?.port(),
                )? {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "loopback radio endpoint does not belong to the launched RTL8821CU helper",
                    ));
                }
                socket.set_nodelay(true)?;
                radio_link::handshake(&mut socket, channel, mac).await?;
                self.bringup_until = None;
                return socket.into_std();
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "RTL8821CU helper initialization timed out",
                ));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    fn signal_stop(&self) {
        // SAFETY: the event handle stays open until Helper is dropped.
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.signal_stop();
        // The released helper only observes its stop event between USB init
        // phases. Keep the original startup window open before forcing an exit.
        let now = Instant::now();
        let deadline = self.bringup_until.unwrap_or(now).max(now) + RTL8821CU_HELPER_CLEANUP;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(30)),
                Err(_) => break,
            }
        }
        tracing::warn!("RTL8821CU helper did not stop gracefully; USB replug may be needed");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn owns_connection(pid: u32, server_port: u16, client_port: u16) -> io::Result<bool> {
    // MIB_TCPTABLE_OWNER_PID consists of a DWORD count followed by six DWORDs
    // per row. A u32 buffer satisfies the table alignment on Windows.
    let mut words = vec![0_u32; 1024];
    for _ in 0..4 {
        let mut bytes = (words.len() * size_of::<u32>()) as u32;
        // SAFETY: the writable table buffer is aligned and sized in bytes.
        let result = unsafe {
            GetExtendedTcpTable(
                words.as_mut_ptr().cast(),
                &mut bytes,
                0,
                u32::from(AF_INET),
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER && bytes <= 4 * 1024 * 1024 {
            words.resize((bytes as usize).div_ceil(size_of::<u32>()), 0);
            continue;
        }
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        const LOOPBACK: u32 = u32::from_ne_bytes([127, 0, 0, 1]);
        const ESTABLISHED: u32 = 5;
        let (rows, _) = words[1..].as_chunks::<6>();
        return Ok(rows.iter().take(words[0] as usize).any(|row| {
            row[0] == ESTABLISHED
                && row[1] == LOOPBACK
                && row[3] == LOOPBACK
                && u16::from_be(row[2] as u16) == server_port
                && u16::from_be(row[4] as u16) == client_port
                && row[5] == pid
        }));
    }
    Err(io::Error::other(
        "could not inspect the RTL8821CU loopback connection",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_the_server_side_of_a_real_windows_loopback_connection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(server).unwrap();
        let (_accepted, client_address) = listener.accept().unwrap();
        assert_eq!(client.local_addr().unwrap(), client_address);
        assert!(owns_connection(std::process::id(), server.port(), client_address.port()).unwrap());
        assert!(!owns_connection(0, server.port(), client_address.port()).unwrap());
    }
}
