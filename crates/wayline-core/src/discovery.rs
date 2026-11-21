//! Device discovery shared by CLI selection and future tray clients.

use crate::sender::{client_config, discover, NearbyReceiver, SendError};
use crate::{
    AirDropService, Identity, MdnsBrowser, NetworkInterface, SendCancellation, SenderIdentity,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Found(NearbyReceiver),
    Updated(NearbyReceiver),
    Removed {
        instance: String,
    },
    ProbeFailed {
        instance: String,
        attempt: u8,
        error: String,
    },
}

struct Probe {
    service: AirDropService,
    receiver: Option<NearbyReceiver>,
    attempts: u8,
    next_attempt: Duration,
    finished: bool,
}

impl Probe {
    fn new(service: AirDropService) -> Self {
        Self {
            service,
            receiver: None,
            attempts: 0,
            next_attempt: Duration::ZERO,
            finished: false,
        }
    }

    fn due(&self, now: Duration) -> bool {
        !self.finished && now >= self.next_attempt
    }

    fn finish(
        &mut self,
        now: Duration,
        result: Result<Option<NearbyReceiver>, SendError>,
    ) -> Option<DiscoveryEvent> {
        self.attempts = self.attempts.saturating_add(1);
        match result {
            Ok(Some(receiver)) => {
                self.receiver = Some(receiver.clone());
                self.finished = true;
                Some(DiscoveryEvent::Found(receiver))
            }
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                // Keep probing while the caller's discovery deadline is active. A receiver
                // can advertise before its TCP listener is ready or briefly refuse a
                // connection; stopping after three failures wastes the remaining timeout.
                // Cap the backoff so late recovery is still detected promptly.
                self.next_attempt = now + Duration::from_secs(u64::from(self.attempts.min(3)));
                Some(DiscoveryEvent::ProbeFailed {
                    instance: self.service.instance.clone(),
                    attempt: self.attempts,
                    error: error.to_string(),
                })
            }
        }
    }

    fn update(&mut self, service: AirDropService) -> Option<DiscoveryEvent> {
        if self.service.address != service.address || self.service.host != service.host {
            let removed = self.receiver.take().map(|_| DiscoveryEvent::Removed {
                instance: self.service.instance.clone(),
            });
            *self = Self::new(service);
            return removed;
        }
        if self.service == service {
            return None;
        }
        self.service = service;
        self.receiver.as_mut().map(|receiver| {
            receiver.service = self.service.clone();
            DiscoveryEvent::Updated(receiver.clone())
        })
    }
}

/// No file offers are sent here. Failed probes retry with bounded backoff until the
/// caller's timeout, and late TXT capability records update an already named device.
pub fn discover_receivers_live(
    interface: NetworkInterface,
    identity: &Identity,
    timeout: Duration,
    ble_wake: bool,
    legacy_ble: bool,
    notify: impl FnMut(DiscoveryEvent),
) -> Result<Vec<NearbyReceiver>, SendError> {
    discover_receivers_live_cancellable(
        interface,
        identity,
        timeout,
        ble_wake,
        legacy_ble,
        &SendCancellation::default(),
        notify,
    )
}

pub fn discover_receivers_live_cancellable(
    interface: NetworkInterface,
    identity: &Identity,
    timeout: Duration,
    ble_wake: bool,
    legacy_ble: bool,
    cancel: &SendCancellation,
    notify: impl FnMut(DiscoveryEvent),
) -> Result<Vec<NearbyReceiver>, SendError> {
    discover_receivers_live_cancellable_with_identity(
        interface,
        identity.into(),
        timeout,
        ble_wake,
        legacy_ble,
        cancel,
        notify,
    )
}

pub fn discover_receivers_live_cancellable_with_identity(
    interface: NetworkInterface,
    identity: SenderIdentity<'_>,
    timeout: Duration,
    ble_wake: bool,
    legacy_ble: bool,
    cancel: &SendCancellation,
    mut notify: impl FnMut(DiscoveryEvent),
) -> Result<Vec<NearbyReceiver>, SendError> {
    if cancel.is_cancelled() {
        return Err(SendError::cancelled());
    }
    if !interface.link_local.is_unicast_link_local() || interface.index == 0 || timeout.is_zero() {
        return Err(SendError::invalid(
            "discovery requires a link-local address, nonzero interface index and positive timeout",
        ));
    }
    let client = client_config(identity.apple)?;
    #[cfg(windows)]
    let mut ble_error = None;
    #[cfg(windows)]
    let mut advert = ble_wake
        .then(|| crate::ble::AirDropAdvert::begin_with_identity(legacy_ble, identity.apple))
        .and_then(|result| match result {
            Ok(advert) => Some(advert),
            Err(error) => {
                ble_error = Some(error);
                None
            }
        });
    #[cfg(windows)]
    let mut advert_started = false;
    #[cfg(not(windows))]
    let _ = (ble_wake, legacy_ble);
    let mut browser = MdnsBrowser::bind(interface.clone(), Some(interface.link_local))?;
    let start = Instant::now();
    let mut next_query = Duration::ZERO;
    let mut probes: BTreeMap<String, Probe> = BTreeMap::new();
    while start.elapsed() < timeout {
        if cancel.is_cancelled() {
            return Err(SendError::cancelled());
        }
        #[cfg(windows)]
        if let Some(publisher) = advert.as_ref().filter(|_| !advert_started) {
            match publisher.ready() {
                Ok(started) => advert_started = started,
                Err(error) => {
                    ble_error = Some(error);
                    advert = None;
                }
            }
        }
        if start.elapsed() >= next_query {
            browser.query()?;
            next_query = start.elapsed() + Duration::from_secs(1);
        }
        let services = browser.recv()?;
        probes.retain(|instance, probe| {
            let present = services.iter().any(|service| &service.instance == instance);
            if !present && probe.receiver.is_some() {
                notify(DiscoveryEvent::Removed {
                    instance: instance.clone(),
                });
            }
            present
        });
        for service in services {
            if let Some(probe) = probes.get_mut(&service.instance) {
                if let Some(event) = probe.update(service) {
                    notify(event);
                }
            } else {
                probes.insert(service.instance.clone(), Probe::new(service));
            }
        }
        // One TLS probe per pass, so pending mDNS updates are drained between peers.
        if let Some(probe) = probes.values_mut().find(|probe| probe.due(start.elapsed())) {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            let result = discover(
                &probe.service,
                &client,
                identity.apple.map(crate::AppleSenderIdentity::record),
                remaining.min(Duration::from_secs(3)),
                cancel,
            );
            cancel.detach();
            if cancel.is_cancelled() {
                return Err(SendError::cancelled());
            }
            if let Some(event) = probe.finish(start.elapsed(), result) {
                notify(event);
            }
        } else {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    // Refresh capabilities even if they arrived during the last TLS operation.
    let services = browser.recv()?;
    for (instance, probe) in &probes {
        if probe.receiver.is_some() && !services.iter().any(|service| &service.instance == instance)
        {
            notify(DiscoveryEvent::Removed {
                instance: instance.clone(),
            });
        }
    }
    let mut result = Vec::new();
    for service in services {
        if let Some(probe) = probes.get_mut(&service.instance) {
            if let Some(event) = probe.update(service) {
                notify(event);
            }
            if let Some(receiver) = &probe.receiver {
                result.push(receiver.clone());
            }
        }
    }
    if cancel.is_cancelled() {
        return Err(SendError::cancelled());
    }
    #[cfg(windows)]
    if result.is_empty() {
        // The last TCP probe can run past the publisher's startup deadline.
        // Resolve its final status before reporting an empty discovery result.
        let error = ble_error.or_else(|| {
            advert
                .as_ref()
                .filter(|_| !advert_started)
                .and_then(|publisher| publisher.ready().err())
        });
        if let Some(error) = error {
            return Err(error);
        }
    }
    Ok(result)
}

/// Names are labels, not device identifiers. Ambiguous matches require a choice.
pub fn receiver_by_name<'a>(
    receivers: &'a [NearbyReceiver],
    name: &str,
) -> Result<&'a NearbyReceiver, SendError> {
    let mut matches = receivers
        .iter()
        .filter(|receiver| receiver.name.eq_ignore_ascii_case(name));
    let receiver = matches.next().ok_or_else(|| {
        SendError::invalid(format!("no AirDrop receiver named \"{name}\" answered"))
    })?;
    if matches.next().is_some() {
        return Err(SendError::invalid(format!(
            "multiple devices are named \"{name}\"; omit --to and select a device by number"
        )));
    }
    Ok(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_discovery_does_not_open_a_network_socket_or_start_ble() {
        let directory = tempfile::tempdir().unwrap();
        let identity = Identity::load_or_create(directory.path(), "test").unwrap();
        let cancel = SendCancellation::default();
        cancel.cancel();
        let error = discover_receivers_live_cancellable(
            NetworkInterface {
                name: "missing".into(),
                index: 0,
                link_local: "::1".parse().unwrap(),
            },
            &identity,
            Duration::from_secs(1),
            true,
            false,
            &cancel,
            |_| panic!("cancelled discovery cannot emit events"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), crate::TransferFailureKind::Cancelled);
    }
    fn service() -> AirDropService {
        AirDropService {
            instance: "phone._airdrop._tcp.local".into(),
            host: "phone.local".into(),
            port: 8770,
            address: "[fe80::1%1]:8770".parse().unwrap(),
            flags: None,
        }
    }
    #[test]
    fn transient_probe_failure_is_retried_and_late_flags_update_named_device() {
        let mut probe = Probe::new(service());
        assert!(probe.due(Duration::ZERO));
        assert!(matches!(
            probe.finish(Duration::ZERO, Err(SendError::invalid("timeout"))),
            Some(DiscoveryEvent::ProbeFailed { attempt: 1, .. })
        ));
        assert!(!probe.due(Duration::from_millis(500)));
        assert!(probe.due(Duration::from_secs(1)));
        let mut receiver = discovered_receiver();
        receiver.receiver_certificate_sha256 = Some([3; 32]);
        assert!(matches!(
            probe.finish(Duration::from_secs(1), Ok(Some(receiver))),
            Some(DiscoveryEvent::Found(_))
        ));
        assert!(!probe.due(Duration::from_secs(10)));
        let mut updated = service();
        updated.flags = Some(0x1b3fb);
        assert!(
            matches!(probe.update(updated), Some(DiscoveryEvent::Updated(receiver))
                if receiver.service.flags == Some(0x1b3fb)
                    && receiver.receiver_certificate_sha256 == Some([3; 32]))
        );
        let mut moved = service();
        moved.address.set_port(8771);
        assert!(matches!(
            probe.update(moved),
            Some(DiscoveryEvent::Removed { .. })
        ));
        assert!(probe.receiver.is_none() && probe.due(Duration::ZERO));
    }

    fn discovered_receiver() -> NearbyReceiver {
        NearbyReceiver {
            name: "iPhone".into(),
            service: service(),
            receiver_certificate_sha256: None,
        }
    }

    #[test]
    fn probe_retries_continue_until_timeout_and_duplicate_names_are_not_selected() {
        let mut probe = Probe::new(service());
        for second in [0, 1, 3, 6, 9, 12] {
            assert!(probe.due(Duration::from_secs(second)));
            probe.finish(
                Duration::from_secs(second),
                Err(SendError::invalid("timeout")),
            );
        }
        assert!(!probe.due(Duration::from_secs(14)));
        assert!(probe.due(Duration::from_secs(15)));
        assert!(matches!(
            probe.finish(Duration::from_secs(15), Ok(Some(discovered_receiver()))),
            Some(DiscoveryEvent::Found(receiver)) if receiver.name == "iPhone"
        ));
        assert!(!probe.due(Duration::from_secs(30)));
        let first = discovered_receiver();
        assert!(receiver_by_name(std::slice::from_ref(&first), "iphone").is_ok());
        let mut second = first.clone();
        second.service.instance = "other._airdrop._tcp.local".into();
        assert!(receiver_by_name(&[first, second], "iPhone").is_err());
    }

    #[test]
    fn long_discovery_timeouts_do_not_overflow_probe_attempts() {
        let mut probe = Probe::new(service());
        probe.attempts = u8::MAX;
        let now = Duration::from_secs(1_000);
        assert!(matches!(
            probe.finish(now, Err(SendError::invalid("connection refused"))),
            Some(DiscoveryEvent::ProbeFailed {
                attempt: u8::MAX,
                ..
            })
        ));
        assert_eq!(probe.next_attempt, now + Duration::from_secs(3));
    }
}
