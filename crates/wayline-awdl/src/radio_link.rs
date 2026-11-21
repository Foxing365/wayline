//! DropBridge radio_link v3 over loopback, with the RTL8821CU radiotap format.
//!
//! The radio remains a separate process. Its USB driver and firmware belong to
//! DropBridge/devourer. This module adapts its wire protocol to Owly's Air trait.

use crate::{driver, invalid};
use owly_awdl::{Frame, MacAddr, TxRate, BSSID};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

const HELLO: u16 = 1;
const RX: u16 = 2;
const TX: u16 = 3;
const SET_BSSID_FILTER: u16 = 5;
const SET_ADDRESS: u16 = 6;
const MAX_PAYLOAD: usize = 16384;
const MAX_QUEUED: usize = 64;

pub async fn handshake(stream: &mut TcpStream, channel: u8, mac: MacAddr) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (kind, payload) = read_message(stream).await?;
        if kind != HELLO || payload.as_slice() != [3, 0, 0, 0, channel, 20, 0, 0] {
            return Err(invalid(
                "unexpected radio_link HELLO (requires v3, selected channel, 20 MHz)",
            ));
        }
        // This order lets the radio program its hardware ACK address and BSSID
        // together when processing SET_ADDRESS.
        stream
            .write_all(&message(SET_BSSID_FILTER, &BSSID.0))
            .await?;
        stream.write_all(&message(SET_ADDRESS, &mac.0)).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "radio_link handshake timed out"))?
}

pub struct Radio {
    receive: mpsc::Receiver<io::Result<driver::Received>>,
    transmit: mpsc::Sender<Vec<u8>>,
    worker: JoinHandle<()>,
}

impl Radio {
    /// Registers a connected, nonblocking socket with the AWDL thread's runtime.
    pub fn from_socket(socket: std::net::TcpStream) -> io::Result<Self> {
        let (mut reader, mut writer) = TcpStream::from_std(socket)?.into_split();
        let (received, receive) = mpsc::channel(MAX_QUEUED);
        let (transmit, mut outgoing) = mpsc::channel::<Vec<u8>>(MAX_QUEUED);
        let worker = tokio::spawn(async move {
            let read = async {
                loop {
                    let (kind, payload) = read_message(&mut reader).await?;
                    if kind != RX {
                        return Err(invalid("unexpected radio_link message after HELLO"));
                    }
                    if let Some(frame) = receive_frame(&payload)? {
                        match received.try_send(Ok(frame)) {
                            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                            Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                        }
                    }
                }
            };
            let write = async {
                while let Some(bytes) = outgoing.recv().await {
                    writer.write_all(&bytes).await?;
                }
                Ok(())
            };
            let result = tokio::select! {
                result = read => result,
                result = write => result,
            };
            if let Err(error) = result {
                let _ = received.send(Err(error)).await;
            }
        });
        Ok(Self {
            receive,
            transmit,
            worker,
        })
    }
}

impl driver::Air for Radio {
    async fn recv(&mut self) -> io::Result<driver::Received> {
        self.receive.recv().await.unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "RTL8821CU radio disconnected",
            ))
        })
    }

    fn send(&mut self, frame: Frame) -> Result<(), Frame> {
        let packet = transmit_frame(&frame);
        self.transmit.try_send(packet).map_err(|_| frame)
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

async fn read_message(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<(u16, Vec<u8>)> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).await?;
    let kind = u16::from_le_bytes([header[0], header[1]]);
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if header[2..4] != [0, 0] || size > MAX_PAYLOAD {
        return Err(invalid("invalid radio_link message header"));
    }
    let mut payload = vec![0; size];
    reader.read_exact(&mut payload).await?;
    Ok((kind, payload))
}

fn message(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend(kind.to_le_bytes());
    bytes.extend([0, 0]);
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    bytes
}

fn receive_frame(payload: &[u8]) -> io::Result<Option<driver::Received>> {
    // DropBridge v0.1.0-internal.1 emits only FLAGS and DBM_ANTSIGNAL.
    let [0, 0, 10, 0, 0x22, 0, 0, 0, flags, signal, frame @ ..] = payload else {
        return Err(invalid("unexpected RTL8821CU receive radiotap format"));
    };
    let fcs_size = if flags & 0x10 != 0 { 4 } else { 0 };
    if flags & 0x40 != 0 || frame.len() < 24 + fcs_size {
        return Ok(None);
    }
    Ok(Some(driver::Received {
        bytes: frame[..frame.len() - fcs_size].to_vec(),
        rssi: Some(*signal as i8),
        received_at: None,
    }))
}

fn transmit_frame(frame: &Frame) -> Vec<u8> {
    // RATE or MCS plus aligned TX_FLAGS. Never pass an FCS to the radio.
    let flags = if frame.no_ack { 0x08 } else { 0 };
    let mut payload = Vec::with_capacity(13 + frame.bytes.len());
    match frame.rate {
        TxRate::Legacy(rate) => payload.extend([0, 0, 12, 0, 0x04, 0x80, 0, 0, rate, 0, flags, 0]),
        TxRate::Ht { mcs, sgi } => payload.extend([
            0,
            0,
            13,
            0,
            0,
            0x80,
            0x08,
            0,
            flags,
            0,
            0x07,
            if sgi { 0x04 } else { 0 },
            mcs,
        ]),
    }
    payload.extend_from_slice(&frame.bytes);
    message(TX, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exchanges_radio_identity_and_converts_bidirectional_frames() {
        use driver::Air;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mac = MacAddr([2, 1, 2, 3, 4, 5]);
        let host = tokio::spawn(async move {
            let mut socket = TcpStream::connect(address).await.unwrap();
            handshake(&mut socket, 149, mac).await.unwrap();
            let mut radio = Radio::from_socket(socket.into_std().unwrap()).unwrap();
            let frame = radio.recv().await.unwrap();
            assert_eq!(frame.bytes, [0xd0; 24]);
            assert_eq!(frame.rssi, Some(-38));
            radio
                .send(Frame {
                    bytes: frame.bytes,
                    to: MacAddr([0xff; 6]),
                    rate: TxRate::BASIC,
                    no_ack: true,
                })
                .unwrap();
            assert!(radio.recv().await.is_err());
        });
        let (mut socket, _) = listener.accept().await.unwrap();
        socket
            .write_all(&message(HELLO, &[3, 0, 0, 0, 149, 20, 0, 0]))
            .await
            .unwrap();
        assert_eq!(
            read_message(&mut socket).await.unwrap(),
            (SET_BSSID_FILTER, BSSID.0.to_vec())
        );
        assert_eq!(
            read_message(&mut socket).await.unwrap(),
            (SET_ADDRESS, mac.0.to_vec())
        );
        let mut payload = vec![0, 0, 10, 0, 0x22, 0, 0, 0, 0x10, 218];
        payload.extend([0xd0; 24]);
        payload.extend([1, 2, 3, 4]);
        // Receive a message split inside its header as TCP may deliver it.
        let bytes = message(RX, &payload);
        socket.write_all(&bytes[..3]).await.unwrap();
        socket.write_all(&bytes[3..]).await.unwrap();
        let (kind, payload) = read_message(&mut socket).await.unwrap();
        assert_eq!(kind, TX);
        assert_eq!(&payload[..12], &[0, 0, 12, 0, 4, 128, 0, 0, 24, 0, 8, 0]);
        assert_eq!(&payload[12..], &[0xd0; 24]);
        // An oversized header must end the session without waiting for its data.
        socket.write_all(&[2, 0, 0, 0, 1, 64, 0, 0]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), host)
            .await
            .unwrap()
            .unwrap();
    }
}
