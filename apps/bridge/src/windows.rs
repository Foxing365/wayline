use std::{
    ffi::OsString,
    fs, io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    time::Duration,
};
use tokio::sync::watch;
use wayline_awdl::{
    driver,
    serial::Radio,
    windows::{feedback, wintun::Wintun},
    MacAddr,
};
use wayline_service::config::{
    validate_rtl8821cu_mtu, validate_rtl8821cu_width_on_channel, RTL8821CU_DEFAULT_MTU,
    RTL8821CU_DEFAULT_WIDTH_MHZ,
};
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{OpenEventW, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE},
};

mod rtl8821cu;
mod timer_resolution;

enum RadioSource {
    Esp32(String),
    Rtl8821cu,
}

struct Options {
    source: RadioSource,
    channel: u8,
    managed: bool,
    rtl_performance: rtl8821cu::PerformanceOptions,
}
impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<Option<Self>> {
        let mut args = args.into_iter();
        let mut source = None;
        let mut channel = 6;
        let mut managed = false;
        let mut rtl_rate_profile = None;
        let mut rtl_timer_1ms = false;
        let mut rtl_mtu = None;
        let mut rtl_width_mhz = None;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => {
                    println!("wayline-bridge (--port COM<n> | --rtl8821cu) [--channel 6|44|149] [--managed]\nRTL8821CU options: --rtl-rate-profile baseline|ht20-mcs4|ht20-mcs7, --rtl-timer-1ms, --rtl-mtu 1440|1450, --rtl-width-mhz 20|40|80 (monitor bandwidth; 40/80 require channel 44 or 149)\nRequires the verified wintun.dll beside the executable. --managed is for service launches.");
                    return Ok(None);
                }
                Some("--port") => {
                    let value = args
                        .next()
                        .ok_or_else(|| invalid("--port requires a value"))?;
                    let port = wayline_service::config::normalize_port(
                        value.to_str().ok_or_else(|| invalid("invalid port"))?,
                    )?;
                    if port == "auto" || source.replace(RadioSource::Esp32(port)).is_some() {
                        return Err(invalid(
                            "select exactly one radio and use an explicit COM port",
                        ));
                    }
                }
                Some("--rtl8821cu") => {
                    if source.replace(RadioSource::Rtl8821cu).is_some() {
                        return Err(invalid("select exactly one radio"));
                    }
                }
                Some("--rtl-rate-profile") => {
                    let profile = args
                        .next()
                        .ok_or_else(|| invalid("--rtl-rate-profile requires a value"))?;
                    let profile = profile
                        .to_str()
                        .ok_or_else(|| invalid("invalid RTL8821CU rate profile"))?
                        .parse()?;
                    if rtl_rate_profile.replace(profile).is_some() {
                        return Err(invalid("select exactly one RTL8821CU rate profile"));
                    }
                }
                Some("--rtl-timer-1ms") => rtl_timer_1ms = true,
                Some("--rtl-mtu") => {
                    let mtu = args
                        .next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                        .ok_or_else(|| invalid("--rtl-mtu requires 1440 or 1450"))?;
                    if rtl_mtu.replace(validate_rtl8821cu_mtu(mtu)?).is_some() {
                        return Err(invalid("select exactly one RTL8821CU MTU"));
                    }
                }
                Some("--rtl-width-mhz") => {
                    let width = args
                        .next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                        .ok_or_else(|| invalid("--rtl-width-mhz requires 20, 40 or 80"))?;
                    if rtl_width_mhz.replace(width).is_some() {
                        return Err(invalid("select exactly one RTL8821CU monitor width"));
                    }
                }
                Some("--channel") => {
                    channel = args
                        .next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                        .ok_or_else(|| invalid("--channel requires 6, 44 or 149"))?;
                }
                Some("--managed") => managed = true,
                _ => {
                    return Err(invalid(format!(
                        "unknown argument: {}",
                        arg.to_string_lossy()
                    )))
                }
            }
        }
        let source = source.ok_or_else(|| invalid("select --port COM<n> or --rtl8821cu"))?;
        if matches!(source, RadioSource::Esp32(_))
            && (rtl_rate_profile.is_some()
                || rtl_timer_1ms
                || rtl_mtu.is_some()
                || rtl_width_mhz.is_some())
        {
            return Err(invalid(
                "--rtl-rate-profile, --rtl-timer-1ms and --rtl-mtu require --rtl8821cu",
            ));
        }
        if ![6, 44, 149].contains(&channel) {
            return Err(invalid("unsupported channel"));
        }
        let rtl_width_mhz = rtl_width_mhz.unwrap_or(RTL8821CU_DEFAULT_WIDTH_MHZ);
        validate_rtl8821cu_width_on_channel(rtl_width_mhz, u16::from(channel))?;
        Ok(Some(Self {
            source,
            channel,
            managed,
            rtl_performance: rtl8821cu::PerformanceOptions {
                rate_profile: rtl_rate_profile.unwrap_or_default(),
                high_resolution_timer: rtl_timer_1ms,
                mtu: rtl_mtu.unwrap_or(RTL8821CU_DEFAULT_MTU),
                width_mhz: rtl_width_mhz as u8,
            },
        }))
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct StopEvent(OwnedHandle);
impl StopEvent {
    fn open() -> io::Result<Option<Self>> {
        let Some(name) = std::env::var_os("WAYLINE_BRIDGE_STOP_EVENT") else {
            return Ok(None);
        };
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
        // SAFETY: name is terminated; ownership of the newly opened handle moves
        // to OwnedHandle exactly once and lasts through all polling.
        let handle = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(Self(unsafe { OwnedHandle::from_raw_handle(handle) })))
    }
    fn signalled(&self) -> io::Result<bool> {
        // SAFETY: the owned event is live; a zero timeout never blocks.
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            _ => Ok(false),
        }
    }
}

pub fn main() -> io::Result<()> {
    let Some(options) = Options::parse(std::env::args_os().skip(1))? else {
        return Ok(());
    };
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .try_init();
    let stop = StopEvent::open()?;
    if options.managed && stop.is_none() {
        return Err(invalid("--managed requires the service stop event"));
    }
    if let Some(event) = &stop {
        if event.signalled()? {
            return Ok(());
        }
    }
    let executable = std::env::current_exe()?;
    let dll = executable.with_file_name("wintun.dll");
    if !dll.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "verified wintun.dll is required beside wayline-bridge.exe",
        ));
    }
    let RadioSource::Esp32(port) = options.source else {
        let helper = executable
            .with_file_name("usb-radio")
            .join("airbridge_radio.exe");
        return rtl8821cu::run(
            options.channel,
            options.managed,
            helper,
            dll,
            stop,
            options.rtl_performance,
        );
    };
    // The service owns the device guard for managed children. Foreground use
    // acquires the same guard before any serial/driver access.
    let _guard = if options.managed {
        None
    } else {
        Some(wayline_service::device_lock::DeviceGuard::try_acquire(
            &port,
        )?)
    };
    if wayline_service::device_lock::has_failed(&port)? {
        return Err(io::Error::other(
            "device readiness is unconfirmed; complete firmware maintenance before starting AWDL",
        ));
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        let stopping = wait_for_stop(&stop);
        tokio::pin!(stopping);
        let target_file = std::env::var_os("WAYLINE_BRIDGE_TARGET_FILE").map(std::path::PathBuf::from);
        let publish_target = move |hello: &wayline_awdl::serial::Hello| {
            if let Some(path) = target_file.as_deref() {
                publish_radio_target(path, &hello.target)?;
            }
            Ok(())
        };
        let (radio,hello)=tokio::select! {
            result=Radio::open_with_target(&port,options.channel,publish_target)=>result?,
            result=&mut stopping=>return result,
        };
        let config=wayline_awdl::configuration(hello.mac,"Wayline",&hello.target,options.channel)?;
        let control=radio.control();
        let (peers,_)=watch::channel(Vec::new());
        let running=driver::spawn(config,move || {
            let net=Wintun::open(&dll,MacAddr(hello.mac))?;
            Ok((radio,net))
        },peers)?;
        let mut failed=running.failed();
        let (shutdown,feedback_stop)=watch::channel(false);
        let feedback=tokio::spawn(async move {
            if let Err(e)=feedback::serve(feedback::PIPE,control,feedback_stop).await {tracing::warn!("board feedback unavailable: {e}");}
        });
        tracing::info!("Rust AWDL ready on {} channel {}",port,options.channel);
        let result=loop {
            if let Some(e)=failed.borrow().as_ref() {break Err(io::Error::other(e.clone()));}
            tokio::select! {
                result=&mut stopping=>break result,
                result=failed.changed()=>{if result.is_err() {break Err(io::Error::other("AWDL worker ended unexpectedly"));}},
            }
        };
        shutdown.send_replace(true);
        let _=feedback.await;
        // Joins AWDL, serial and Wintun workers before returning to the supervisor.
        running.stop();
        tracing::info!("Rust AWDL stopped; radio and awdl0 released");
        result
    })
}

fn publish_radio_target(path: &Path, target: &str) -> io::Result<()> {
    if !matches!(target, "esp32s3" | "esp32c5" | "rtl8821cu") {
        return Err(invalid("unsupported radio target"));
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, target.as_bytes())?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

async fn wait_for_stop(event: &Option<StopEvent>) -> io::Result<()> {
    let mut poll = tokio::time::interval(Duration::from_millis(50));
    let ctrl = tokio::signal::ctrl_c();
    tokio::pin!(ctrl);
    loop {
        tokio::select! {
            result=&mut ctrl=>return result,
            _=poll.tick()=>if let Some(event)=event {if event.signalled()? {return Ok(());}},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayline_service::config::Rtl8821cuRateProfile;

    fn args(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }
    #[test]
    fn rejects_ambiguous_or_unsupported_arguments_before_opening_devices() {
        for a in [
            vec![],
            vec!["--port", "auto"],
            vec!["--port", "COM0"],
            vec!["--rtl8821cu", "--port", "COM1"],
            vec!["--port", "COM1", "--channel", "5"],
            vec!["--port", "COM1", "--unknown"],
        ] {
            assert!(Options::parse(args(&a)).is_err());
        }
        let o = Options::parse(args(&["--port", "com004", "--channel", "149", "--managed"]))
            .unwrap()
            .unwrap();
        assert!(matches!(o.source, RadioSource::Esp32(ref port) if port == "COM4"));
        assert_eq!(o.channel, 149);
        assert!(o.managed);
    }

    #[test]
    fn accepts_only_explicit_rtl8821cu_experiment_options() {
        let defaults = Options::parse(args(&["--rtl8821cu"])).unwrap().unwrap();
        assert_eq!(
            defaults.rtl_performance.rate_profile,
            Rtl8821cuRateProfile::Baseline
        );
        assert!(!defaults.rtl_performance.high_resolution_timer);
        assert_eq!(defaults.rtl_performance.mtu, RTL8821CU_DEFAULT_MTU);
        assert_eq!(defaults.rtl_performance.width_mhz, 20);
        for profile in [
            Rtl8821cuRateProfile::Baseline,
            Rtl8821cuRateProfile::Ht20Mcs4,
            Rtl8821cuRateProfile::Ht20Mcs7,
        ] {
            let options = Options::parse(args(&[
                "--rtl-rate-profile",
                profile.as_str(),
                "--rtl8821cu",
                "--rtl-timer-1ms",
                "--rtl-mtu",
                "1440",
            ]))
            .unwrap()
            .unwrap();
            assert!(matches!(options.source, RadioSource::Rtl8821cu));
            assert_eq!(options.rtl_performance.rate_profile, profile);
            assert!(options.rtl_performance.high_resolution_timer);
            assert_eq!(options.rtl_performance.mtu, 1440);
        }
        for (requested, width) in [("40", 40), ("80", 80)] {
            let options = Options::parse(args(&[
                "--rtl-width-mhz",
                requested,
                "--channel",
                "149",
                "--rtl8821cu",
            ]))
            .unwrap()
            .unwrap();
            assert_eq!(options.rtl_performance.width_mhz, width);
        }
        for invalid in [
            &["--port", "COM1", "--rtl-rate-profile", "baseline"][..],
            &["--port", "COM1", "--rtl-timer-1ms"],
            &["--port", "COM1", "--rtl-mtu", "1440"],
            &["--port", "COM1", "--rtl-width-mhz", "20"],
            &["--rtl8821cu", "--rtl-width-mhz", "80"],
            &["--rtl8821cu", "--channel", "149", "--rtl-width-mhz", "160"],
            &["--rtl8821cu", "--rtl-rate-profile", "ht40-mcs7"],
            &["--rtl8821cu", "--rtl-mtu", "1500"],
            &["--rtl8821cu", "--rtl-rate-profile"],
            &[
                "--rtl8821cu",
                "--rtl-rate-profile",
                "ht20-mcs7",
                "--rtl-rate-profile",
                "baseline",
            ],
        ] {
            assert!(Options::parse(args(invalid)).is_err(), "{invalid:?}");
        }
    }
}
