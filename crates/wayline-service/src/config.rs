use serde::{Deserialize, Serialize};
use std::{fs, io, io::Write, path::Path, str::FromStr, time::Duration};

pub const RTL8821CU_PORT: &str = "RTL8821CU";
pub const RTL8821CU_STARTUP: Duration = Duration::from_secs(150);
pub const RTL8821CU_HELPER_CLEANUP: Duration = Duration::from_secs(12);
pub const RTL8821CU_OPERATION: Duration = Duration::from_secs(170);
pub const RTL8821CU_DEFAULT_MTU: u32 = 1450;
pub const RTL8821CU_DEFAULT_WIDTH_MHZ: u16 = 20;
pub const SET_RTL8821CU_WIDTH: &str = "set-rtl8821cu-width";

pub fn validate_rtl8821cu_width(width_mhz: u16) -> io::Result<u16> {
    if matches!(width_mhz, RTL8821CU_DEFAULT_WIDTH_MHZ | 40 | 80) {
        Ok(width_mhz)
    } else {
        Err(invalid("RTL8821CU monitor width must be 20, 40 or 80 MHz"))
    }
}

pub fn validate_rtl8821cu_width_on_channel(width_mhz: u16, channel: u16) -> io::Result<()> {
    validate_rtl8821cu_width(width_mhz)?;
    if width_mhz != RTL8821CU_DEFAULT_WIDTH_MHZ && !matches!(channel, 44 | 149) {
        return Err(invalid("40 and 80 MHz require channel 44 or 149"));
    }
    Ok(())
}

pub fn validate_rtl8821cu_mtu(mtu: u32) -> io::Result<u32> {
    if matches!(mtu, 1440 | RTL8821CU_DEFAULT_MTU) {
        Ok(mtu)
    } else {
        Err(invalid("RTL8821CU MTU must be 1440 or 1450"))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RadioBackend {
    #[default]
    Esp32,
    Rtl8821cu,
}

impl RadioBackend {
    pub(crate) fn stop_grace(self) -> Duration {
        match self {
            Self::Esp32 => Duration::from_secs(5),
            Self::Rtl8821cu => RTL8821CU_OPERATION,
        }
    }
}

/// Optional RTL8821CU transmit profiles for controlled on-device comparisons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rtl8821cuRateProfile {
    #[default]
    Baseline,
    Ht20Mcs4,
    Ht20Mcs7,
}

impl Rtl8821cuRateProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Ht20Mcs4 => "ht20-mcs4",
            Self::Ht20Mcs7 => "ht20-mcs7",
        }
    }

    pub fn preferred_mcs(self) -> Option<u8> {
        match self {
            Self::Baseline => None,
            Self::Ht20Mcs4 => Some(4),
            Self::Ht20Mcs7 => Some(7),
        }
    }
}

impl FromStr for Rtl8821cuRateProfile {
    type Err = io::Error;

    fn from_str(value: &str) -> io::Result<Self> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "ht20-mcs4" => Ok(Self::Ht20Mcs4),
            "ht20-mcs7" => Ok(Self::Ht20Mcs7),
            _ => Err(invalid(
                "RTL8821CU rate profile must be baseline, ht20-mcs4 or ht20-mcs7",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    #[serde(alias = "Port")]
    pub port: String,
    #[serde(alias = "Channel")]
    pub channel: u16,
    #[serde(alias = "Enabled")]
    pub enabled: bool,
    pub bridge_path: Option<String>,
    pub radio_backend: RadioBackend,
    pub rtl8821cu_rate_profile: Rtl8821cuRateProfile,
    pub rtl8821cu_high_resolution_timer: bool,
    pub rtl8821cu_mtu: u32,
    pub rtl8821cu_width_mhz: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: "auto".into(),
            channel: 6,
            enabled: true,
            bridge_path: None,
            radio_backend: RadioBackend::Esp32,
            rtl8821cu_rate_profile: Rtl8821cuRateProfile::Baseline,
            rtl8821cu_high_resolution_timer: false,
            rtl8821cu_mtu: RTL8821CU_DEFAULT_MTU,
            rtl8821cu_width_mhz: RTL8821CU_DEFAULT_WIDTH_MHZ,
        }
    }
}

impl Config {
    pub fn validate(&mut self) -> io::Result<()> {
        self.port = normalize_port(&self.port)?;
        if self.radio_backend == RadioBackend::Rtl8821cu && self.port != "auto" {
            return Err(invalid("RTL8821CU uses port=auto and connects over WinUSB"));
        }
        if self.radio_backend != RadioBackend::Rtl8821cu
            && (self.rtl8821cu_rate_profile != Rtl8821cuRateProfile::Baseline
                || self.rtl8821cu_high_resolution_timer
                || self.rtl8821cu_mtu != RTL8821CU_DEFAULT_MTU
                || self.rtl8821cu_width_mhz != RTL8821CU_DEFAULT_WIDTH_MHZ)
        {
            return Err(invalid(
                "RTL8821CU performance options require radioBackend=rtl8821cu",
            ));
        }
        validate_rtl8821cu_mtu(self.rtl8821cu_mtu)?;
        // OWL's AWDL channel table and the ESP32 firmware support these channels.
        if !matches!(self.channel, 6 | 44 | 149) {
            return Err(invalid("channel must be 6, 44 or 149"));
        }
        validate_rtl8821cu_width_on_channel(self.rtl8821cu_width_mhz, self.channel)?;
        if let Some(path) = &self.bridge_path {
            if path.is_empty() || path.contains('\0') || !Path::new(path).is_absolute() {
                return Err(invalid("bridgePath must be an absolute executable path"));
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let mut config: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut valid = self.clone();
        valid.validate()?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid("config path has no parent"))?;
        fs::create_dir_all(parent)?;
        // Persist uses an atomic replacement on Windows too; a failed write cannot truncate
        // the previous config. The temporary file stays on the same volume.
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, &valid)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

pub fn normalize_port(port: &str) -> io::Result<String> {
    let port = port.trim().to_ascii_uppercase();
    if port == "AUTO" {
        return Ok("auto".into());
    }
    if let Some(number) = port.strip_prefix("COM") {
        if !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()) {
            if let Ok(value @ 1..=65535) = number.parse::<u16>() {
                return Ok(format!("COM{value}"));
            }
        }
    }
    Err(invalid(
        "port must be auto or COM followed by a positive port number",
    ))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl8821cu_performance_options_round_trip_with_compatible_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"radioBackend":"rtl8821cu","channel":149}"#).unwrap();
        let mut config = Config::load(&path).unwrap();
        assert_eq!(
            config.rtl8821cu_rate_profile,
            Rtl8821cuRateProfile::Baseline
        );
        assert!(!config.rtl8821cu_high_resolution_timer);
        assert_eq!(config.rtl8821cu_mtu, RTL8821CU_DEFAULT_MTU);
        for profile in [
            Rtl8821cuRateProfile::Ht20Mcs4,
            Rtl8821cuRateProfile::Ht20Mcs7,
        ] {
            config.rtl8821cu_rate_profile = profile;
            config.rtl8821cu_high_resolution_timer = true;
            config.rtl8821cu_mtu = 1440;
            config.save(&path).unwrap();
            let json: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(json["rtl8821cuRateProfile"], profile.as_str());
            assert_eq!(json["rtl8821cuHighResolutionTimer"], true);
            assert_eq!(json["rtl8821cuMtu"], 1440);
            assert_eq!(Config::load(&path).unwrap(), config);
        }

        let previous = fs::read(&path).unwrap();
        config.radio_backend = RadioBackend::Esp32;
        assert!(config.save(&path).is_err());
        config.rtl8821cu_rate_profile = Rtl8821cuRateProfile::Baseline;
        assert!(config.save(&path).is_err());
        config.rtl8821cu_high_resolution_timer = false;
        assert!(config.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        fs::write(
            &path,
            r#"{"radioBackend":"rtl8821cu","rtl8821cuRateProfile":"vht80"}"#,
        )
        .unwrap();
        assert!(Config::load(&path).is_err());
        fs::write(&path, r#"{"radioBackend":"rtl8821cu","rtl8821cuMtu":1500}"#).unwrap();
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn rtl8821cu_monitor_width_requires_a_supported_five_ghz_channel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"radioBackend":"rtl8821cu","channel":149}"#).unwrap();
        let mut config = Config::load(&path).unwrap();
        assert_eq!(config.rtl8821cu_width_mhz, 20);
        for width in [40, 80] {
            config.rtl8821cu_width_mhz = width;
            config.save(&path).unwrap();
            assert_eq!(Config::load(&path).unwrap().rtl8821cu_width_mhz, width);
        }
        config.channel = 6;
        assert!(config.save(&path).is_err());
        config.channel = 149;
        config.radio_backend = RadioBackend::Esp32;
        assert!(config.save(&path).is_err());
        config.radio_backend = RadioBackend::Rtl8821cu;
        config.rtl8821cu_width_mhz = 160;
        assert!(config.save(&path).is_err());
        assert_eq!(Config::load(&path).unwrap().rtl8821cu_width_mhz, 80);
    }

    #[test]
    fn bridge_path_is_optional_absolute_and_round_trips() {
        let mut config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.bridge_path.is_none());
        config.bridge_path = Some("relative.exe".into());
        assert!(config.validate().is_err());
        config.bridge_path = Some(
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        config.validate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }
    #[test]
    fn reads_pascal_case_config_with_utf8_bom_then_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            b"\xef\xbb\xbf{\"Port\":\"com004\",\"Channel\":44,\"Enabled\":false,\"OwlPath\":null}",
        )
        .unwrap();
        let mut config = Config::load(&path).unwrap();
        assert_eq!(config.port, "COM4");
        assert!(!config.enabled);

        config.channel = 149;
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        let previous = fs::read(&path).unwrap();
        config.channel = 7;
        assert!(config.save(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), previous);
    }

    #[test]
    fn missing_defaults_but_invalid_config_is_not_silently_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        fs::write(&path, "{bad").unwrap();
        assert!(Config::load(&path).is_err());
        for port in [
            "COM0",
            "COM-1",
            "COM65536",
            "COM1 -x",
            "COM¹",
            "\\\\.\\pipe\\foo",
        ] {
            assert!(normalize_port(port).is_err(), "{port}");
        }
    }
}
