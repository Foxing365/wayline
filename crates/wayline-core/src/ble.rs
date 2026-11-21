//! AirDrop wake advertisement sent while discovering nearby devices.

use std::time::{Duration, Instant};
use windows::{
    Devices::Bluetooth::Advertisement::{
        BluetoothLEAdvertisement, BluetoothLEAdvertisementPublisher,
        BluetoothLEAdvertisementPublisherStatus, BluetoothLEManufacturerData,
    },
    Storage::Streams::DataWriter,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

use crate::{sender::SendError, AppleSenderIdentity};

// RoUninitialize must run on the thread which initialized this apartment.
struct Apartment(std::marker::PhantomData<std::rc::Rc<()>>);

impl Apartment {
    fn enter() -> Result<Self, SendError> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| SendError::invalid(format!("initialize Windows Runtime: {error}")))?;
        Ok(Self(std::marker::PhantomData))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

/// Owns the Windows BLE publisher. It stops advertising when dropped.
pub struct AirDropAdvert {
    publisher: BluetoothLEAdvertisementPublisher,
    startup_deadline: Instant,
    _apartment: Apartment,
}

impl AirDropAdvert {
    pub fn start(legacy_shape: bool) -> Result<Self, SendError> {
        Self::start_with_identity(legacy_shape, None)
    }

    pub(crate) fn start_with_identity(
        legacy_shape: bool,
        identity: Option<&AppleSenderIdentity>,
    ) -> Result<Self, SendError> {
        let advert = Self::begin_with_identity(legacy_shape, identity)?;
        while !advert.ready()? {
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(advert)
    }

    pub(crate) fn begin_with_identity(
        legacy_shape: bool,
        identity: Option<&AppleSenderIdentity>,
    ) -> Result<Self, SendError> {
        let apartment = Apartment::enter()?;
        let mut tag = [0; 3];
        getrandom::fill(&mut tag).map_err(|e| SendError::invalid(format!("BLE tag: {e}")))?;
        let payload = continuity_payload(
            legacy_shape,
            tag,
            identity.map(AppleSenderIdentity::contact_prefixes),
        );

        // Release temporary WinRT objects before the apartment can be dropped on
        // a later publisher startup error.
        let publisher = {
            let writer = DataWriter::new()
                .map_err(|error| SendError::invalid(format!("create BLE data writer: {error}")))?;
            writer.WriteBytes(&payload).map_err(|error| {
                SendError::invalid(format!("write AirDrop BLE payload: {error}"))
            })?;
            let buffer = writer.DetachBuffer().map_err(|error| {
                SendError::invalid(format!("finish AirDrop BLE payload: {error}"))
            })?;
            let manufacturer = BluetoothLEManufacturerData::Create(0x004c, &buffer)
                .map_err(|error| SendError::invalid(format!("create Apple BLE data: {error}")))?;
            let advertisement = BluetoothLEAdvertisement::new().map_err(|error| {
                SendError::invalid(format!("create BLE advertisement: {error}"))
            })?;
            advertisement
                .ManufacturerData()
                .and_then(|data| data.Append(&manufacturer))
                .map_err(|error| SendError::invalid(format!("add Apple BLE data: {error}")))?;
            BluetoothLEAdvertisementPublisher::Create(&advertisement)
                .map_err(|error| SendError::invalid(format!("create BLE publisher: {error}")))?
        };
        let advert = Self {
            publisher,
            startup_deadline: Instant::now() + Duration::from_secs(5),
            _apartment: apartment,
        };
        advert
            .publisher
            .Start()
            .map_err(|error| SendError::invalid(format!("start AirDrop BLE advert: {error}")))?;
        Ok(advert)
    }

    pub(crate) fn ready(&self) -> Result<bool, SendError> {
        let status = self
            .publisher
            .Status()
            .map_err(|error| SendError::invalid(format!("read BLE publisher status: {error}")))?;
        if status == BluetoothLEAdvertisementPublisherStatus::Started {
            return Ok(true);
        }
        if status == BluetoothLEAdvertisementPublisherStatus::Aborted
            || Instant::now() >= self.startup_deadline
        {
            return Err(SendError::invalid(format!(
                "AirDrop BLE advertisement did not start (status {status:?})"
            )));
        }
        Ok(false)
    }
}

fn continuity_payload(legacy_shape: bool, tag: [u8; 3], prefixes: Option<[[u8; 2]; 3]>) -> Vec<u8> {
    let mut advert = owly_airdrop::proto::ble::Advert::anonymous(tag);
    if let Some([phone, email, email2]) = prefixes {
        advert.phone = phone;
        advert.email = email;
        advert.email2 = email2;
    }
    let mut payload = advert.manufacturer_data();
    if legacy_shape {
        payload[2..6].fill(0);
        payload[10] = 1;
    }
    payload
}

impl Drop for AirDropAdvert {
    fn drop(&mut self) {
        let _ = self.publisher.Stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuity_payload_matches_current_and_legacy_airdrop_shapes() {
        let current = continuity_payload(false, [0; 3], None);
        assert_eq!(&current[..2], &[0x05, 18]);
        assert_eq!(&current[2..10], &[0x40, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&current[10..], &[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

        let legacy = continuity_payload(true, [0; 3], None);
        assert_eq!(&legacy[..2], &[0x05, 18]);
        assert_eq!(&legacy[2..10], &[0; 8]);
        assert_eq!(&legacy[10..], &[0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn includes_only_the_validated_phone_and_email_prefixes() {
        let contact = [[1, 2], [3, 4], [5, 6]];
        for legacy in [false, true] {
            let payload = continuity_payload(legacy, [0; 3], Some(contact));
            assert_eq!(&payload[11..19], &[0, 0, 1, 2, 3, 4, 5, 6]);
        }
    }
}
