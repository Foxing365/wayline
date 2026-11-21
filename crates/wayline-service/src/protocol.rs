use serde::{Deserialize, Serialize};

pub const SET_RADIO: &str = "set-radio";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(alias = "Cmd")]
    pub cmd: String,
    #[serde(alias = "Port")]
    pub port: Option<String>,
    #[serde(alias = "Channel")]
    pub channel: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtl8821cu_width_mhz: Option<u16>,
    #[serde(alias = "Exe")]
    pub exe: Option<String>,
}

impl Request {
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Rtl8821cuStartupPhase {
    BridgeLaunch,
    UsbHelperInitialization,
    HelloVerification,
    WintunSetup,
    Ready,
}

impl Rtl8821cuStartupPhase {
    pub(crate) fn percent(self) -> u8 {
        match self {
            Self::BridgeLaunch => 5,
            Self::UsbHelperInitialization => 30,
            Self::HelloVerification => 65,
            Self::WintunSetup => 85,
            Self::Ready => 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rtl8821cuStartup {
    pub phase: Rtl8821cuStartupPhase,
    pub percent: u8,
    pub elapsed_seconds: u64,
}

impl Rtl8821cuStartup {
    pub(crate) fn new(phase: Rtl8821cuStartupPhase, elapsed_seconds: u64) -> Self {
        Self {
            phase,
            percent: phase.percent(),
            elapsed_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub port: Option<String>,
    pub channel: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtl8821cu_width_mhz: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtl8821cu_startup: Option<Rtl8821cuStartup>,
    #[serde(default)]
    pub target: Option<String>,
    pub awdl0: bool,
    pub error: Option<String>,
    pub enabled: bool,
}

impl Status {
    pub fn uses_rtl8821cu(&self) -> bool {
        self.port.as_deref() == Some(crate::config::RTL8821CU_PORT)
    }

    pub fn with_error(mut self, error: impl ToString) -> Self {
        self.error = Some(error.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_camel_case_and_pascal_case_requests() {
        for json in [
            r#"{"cmd":"set","port":"COM4","channel":6}"#,
            r#"{"Cmd":"set","Port":"COM4","Channel":6}"#,
        ] {
            let req: Request = serde_json::from_str(json).unwrap();
            assert_eq!(req.cmd, "set");
            assert_eq!(req.port.as_deref(), Some("COM4"));
        }
        let status = Status {
            state: "stopped".into(),
            port: None,
            channel: 6,
            rtl8821cu_width_mhz: None,
            rtl8821cu_startup: None,
            target: None,
            awdl0: false,
            error: None,
            enabled: false,
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["state"], "stopped");
        assert!(value.get("State").is_none());
    }

    #[test]
    fn usb_status_fields_do_not_break_older_status_messages() {
        let legacy = r#"{"state":"running","port":"RTL8821CU","channel":149,"target":"rtl8821cu","awdl0":true,"error":null,"enabled":true}"#;
        let mut status: Status = serde_json::from_str(legacy).unwrap();
        assert!(status.uses_rtl8821cu());
        assert_eq!(status.rtl8821cu_width_mhz, None);
        assert_eq!(status.rtl8821cu_startup, None);
        status.rtl8821cu_width_mhz = Some(80);
        status.rtl8821cu_startup = Some(Rtl8821cuStartup::new(
            Rtl8821cuStartupPhase::UsbHelperInitialization,
            75,
        ));
        let status = serde_json::to_value(status).unwrap();
        assert_eq!(status["rtl8821cuWidthMhz"], 80);
        assert_eq!(
            status["rtl8821cuStartup"],
            serde_json::json!({
                "phase": "usbHelperInitialization",
                "percent": 30,
                "elapsedSeconds": 75,
            })
        );

        let mut request = Request::new(crate::config::SET_RTL8821CU_WIDTH);
        request.rtl8821cu_width_mhz = Some(40);
        assert_eq!(
            serde_json::to_value(request).unwrap()["rtl8821cuWidthMhz"],
            40
        );

        let request: Request =
            serde_json::from_str(r#"{"cmd":"set-radio","channel":44,"rtl8821cuWidthMhz":40}"#)
                .unwrap();
        assert_eq!(request.cmd, SET_RADIO);
        assert_eq!(request.channel, Some(44));
        assert_eq!(request.rtl8821cu_width_mhz, Some(40));
    }
}
