//! Real Windows-compatible TLS and protocol regressions; no radio or service.
use owly_airdrop::{
    driver::sender::prepare,
    proto::{
        archive::compress::Compressor,
        http,
        receiver::{Config, Connection, Event},
        sender::{self, ResponseReader},
    },
};
use std::{
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};
use wayline_core::*;

fn options(format: SendFormat) -> SendOptions {
    SendOptions {
        sender_name: "Windows 测试".into(),
        sender_model: "Windows".into(),
        target_name: "receiver".into(),
        timeout: Duration::from_secs(5),
        format,
        ble_wake: false,
        legacy_ble: false,
    }
}
fn identity(dir: &std::path::Path) -> Identity {
    Identity::load_or_create(dir.join("id"), "Wayline").unwrap()
}
fn selected(addr: std::net::SocketAddrV6) -> NearbyReceiver {
    NearbyReceiver {
        name: "receiver".into(),
        receiver_certificate_sha256: None,
        service: AirDropService {
            instance: "receiver._airdrop._tcp.local".into(),
            host: "localhost".into(),
            port: addr.port(),
            address: addr,
            flags: Some(2),
        },
    }
}
fn start(
    core: Arc<ReceiverCore>,
    id: &Identity,
) -> (
    NearbyReceiver,
    thread::JoinHandle<Result<Vec<ReceiverEvent>, wayline_core::ReceiverError>>,
) {
    let listener = TcpListener::bind("[::1]:0").unwrap();
    let SocketAddr::V6(addr) = listener.local_addr().unwrap() else {
        panic!()
    };
    let config = server_config(id).unwrap();
    let worker = thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        ReceiverCore::serve_tls_connection(s, config, core)
    });
    (selected(addr), worker)
}

#[test]
fn tls_round_trips_both_compressions_empty_unicode_and_directory_files() {
    for format in [SendFormat::Gzip, SendFormat::DvZip] {
        let root = tempfile::tempdir().unwrap();
        let id = identity(root.path());
        let folder = root.path().join("文档");
        fs::create_dir(&folder).unwrap();
        let mut data = vec![0; 2 * 1024 * 1024];
        getrandom::fill(&mut data).unwrap();
        fs::write(folder.join("中文.bin"), &data).unwrap();
        fs::write(folder.join("empty"), []).unwrap();
        let dest = root.path().join("received");
        let core = Arc::new(ReceiverCore::new(
            ReceiverOptions::new("receiver", "Windows", &dest).with_accept_all(),
        ));
        let (peer, worker) = start(core.clone(), &id);
        let mut events = Vec::new();
        let report =
            send_to_receiver(&peer, &id, &[folder], &options(format), |e| events.push(e)).unwrap();
        let received = worker.join().unwrap().unwrap();
        assert_eq!(fs::read(dest.join("文档/中文.bin")).unwrap(), data);
        assert_eq!(fs::metadata(dest.join("文档/empty")).unwrap().len(), 0);
        assert_eq!(report.source_bytes, data.len() as u64);
        assert!(matches!(events.last(), Some(SendEvent::Completed(_))));
        assert!(received
            .iter()
            .any(|e| matches!(e,ReceiverEvent::Receiving{received,..} if *received>0)));
        assert!(matches!(
            received.last(),
            Some(ReceiverEvent::Received { .. })
        ));
        assert!(!core.has_active_transfers());
    }
}

#[test]
fn rejection_and_upload_cancellation_leave_no_published_files() {
    for decline in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let id = identity(root.path());
        let file = root.path().join("data.bin");
        let mut data = vec![0; 4 * 1024 * 1024];
        getrandom::fill(&mut data).unwrap();
        fs::write(&file, data).unwrap();
        let dest = root.path().join("received");
        let mut opts = ReceiverOptions::new("receiver", "Windows", &dest);
        opts.accept_all = !decline;
        let core = Arc::new(ReceiverCore::new(opts));
        let (peer, worker) = start(core.clone(), &id);
        let cancel = SendCancellation::default();
        let result = send_to_receiver_cancellable(
            &peer,
            &id,
            &[file],
            &options(SendFormat::DvZip),
            &cancel,
            |e| {
                if !decline && matches!(e,SendEvent::Uploading{sent,..} if sent>0) {
                    cancel.cancel();
                }
            },
        );
        assert_eq!(
            result.unwrap_err().kind(),
            if decline {
                TransferFailureKind::Declined
            } else {
                TransferFailureKind::Cancelled
            }
        );
        let _ = worker.join().unwrap();
        assert!(!core.has_active_transfers());
        assert!(!dest.exists() || fs::read_dir(dest).unwrap().next().is_none());
    }
}

fn protocol_upload(kind: Compressor, chunked: bool) -> (tempfile::TempDir, Vec<u8>, Vec<u8>) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("a.txt");
    fs::write(&file, b"verified").unwrap();
    let (offer, mut archive, len) = prepare(&[file], kind).unwrap();
    let mut body = Vec::new();
    archive.read_to_end(&mut body).unwrap();
    let cfg = sender::Config::new("sender", "abcdef123456");
    let ask = sender::ask_request(&cfg, "TEST-ID", &offer);
    let length = len.to_string();
    let mut headers = vec![
        ("Content-Type", kind.content_type()),
        ("TransferID", "TEST-ID"),
    ];
    if chunked {
        headers.push(("Transfer-Encoding", "chunked"));
    } else {
        headers.push(("Content-Length", &length));
    }
    let head = http::post_head("/Upload", &headers);
    let mut wire = head;
    wire.extend(if chunked { http::chunk(&body) } else { body });
    (root, ask, wire)
}
fn ask(connection: &mut Connection, bytes: &[u8]) {
    connection.receive(bytes);
    let mut asked = false;
    while let Some(e) = connection.poll() {
        if let Event::Ask(_) = e {
            asked = true;
            connection.accept();
        }
    }
    assert!(asked);
}
fn drain(connection: &mut Connection) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(e) = connection.poll() {
        out.push(e);
    }
    out
}

#[test]
fn strict_upload_waits_for_http_completion_and_checks_compression_checksum() {
    for kind in [Compressor::Gzip, Compressor::DvZip] {
        let (_root, offer, mut wire) = protocol_upload(kind, true);
        let mut c = Connection::new(Arc::new(Config::new("receiver")));
        ask(&mut c, &offer);
        c.receive(&wire);
        assert!(!drain(&mut c)
            .iter()
            .any(|e| matches!(e, Event::UploadComplete)));
        assert!(!c.finish_validated_dvzip_boundary());
        c.receive(b"0\r\n\r\n");
        assert!(drain(&mut c)
            .iter()
            .any(|e| matches!(e, Event::UploadComplete)));
        // A valid archive does not excuse a malformed HTTP trailer.
        let mut c = Connection::new(Arc::new(Config::new("receiver")));
        ask(&mut c, &offer);
        wire.extend(b"0\r\nbad header\r\n\r\n");
        c.receive(&wire);
        let events = drain(&mut c);
        assert!(events.iter().any(|e| matches!(e, Event::UploadFailed(_))));
        assert!(!events.iter().any(|e| matches!(e, Event::UploadComplete)));
    }
    let (_root, offer, mut wire) = protocol_upload(Compressor::Gzip, false);
    let pos = wire.len() - 8;
    wire[pos] ^= 0xff;
    let mut c = Connection::new(Arc::new(Config::new("receiver")));
    ask(&mut c, &offer);
    c.receive(&wire);
    let events = drain(&mut c);
    assert!(events.iter().any(|e| matches!(e, Event::UploadFailed(_))));
    assert!(!events.iter().any(|e| matches!(e, Event::UploadComplete)));
}

#[test]
fn dvzip_boundary_requires_explicit_policy_complete_chunk_and_no_tail() {
    let (_root, offer, wire) = protocol_upload(Compressor::DvZip, true);
    for tail in [b"".as_slice(), b"1", b"1\r\nx"] {
        let mut cfg = Config::new("receiver");
        cfg.allow_dvzip_archive_boundary = true;
        let mut c = Connection::new(Arc::new(cfg));
        ask(&mut c, &offer);
        c.receive(&wire);
        drain(&mut c);
        c.receive(tail);
        drain(&mut c);
        assert_eq!(c.finish_validated_dvzip_boundary(), tail.is_empty());
    }
    let mut cfg = Config::new("receiver");
    cfg.allow_dvzip_archive_boundary = true;
    let mut c = Connection::new(Arc::new(cfg));
    ask(&mut c, &offer);
    c.receive(&wire[..wire.len() - 1]);
    drain(&mut c);
    assert!(!c.finish_validated_dvzip_boundary());
}

#[test]
fn truncated_upload_and_unauthorized_upload_never_complete() {
    let (_root, offer, wire) = protocol_upload(Compressor::DvZip, false);
    let mut c = Connection::new(Arc::new(Config::new("receiver")));
    c.receive(&wire);
    assert!(!drain(&mut c)
        .iter()
        .any(|e| matches!(e, Event::UploadStarted { .. } | Event::UploadComplete)));
    let mut c = Connection::new(Arc::new(Config::new("receiver")));
    ask(&mut c, &offer);
    c.receive(&wire[..wire.len() - 3]);
    drain(&mut c);
    c.receive_eof();
    assert!(drain(&mut c)
        .iter()
        .any(|e| matches!(e, Event::UploadFailed(_))));
}

#[test]
fn a_complete_empty_archive_does_not_satisfy_an_offered_file() {
    use owly_airdrop::proto::archive::{compress::Encoder, cpio};
    let (_root, offer, _) = protocol_upload(Compressor::Gzip, false);
    let mut encoder = Encoder::new(Compressor::Gzip, flate2::Compression::fast());
    let mut body = encoder.write(&cpio::odc_trailer());
    body.extend(encoder.finish());
    let len = body.len().to_string();
    let mut wire = http::post_head(
        "/Upload",
        &[
            ("Content-Type", "application/x-cpio"),
            ("TransferID", "TEST-ID"),
            ("Content-Length", &len),
        ],
    );
    wire.extend(body);
    let mut c = Connection::new(Arc::new(Config::new("receiver")));
    ask(&mut c, &offer);
    c.receive(&wire);
    let events = drain(&mut c);
    assert!(events.iter().any(|e| matches!(e, Event::UploadFailed(_))));
    assert!(!events.iter().any(|e| matches!(e, Event::UploadComplete)));
}

#[test]
fn cancelled_archive_preparation_stops_between_blocks() {
    use std::cell::Cell;
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("large");
    fs::write(&file, vec![0; 4 * 1024 * 1024]).unwrap();
    let calls = Cell::new(0);
    let result =
        owly_airdrop::driver::sender::prepare_cancellable(&[file], Compressor::Gzip, &|| {
            calls.set(calls.get() + 1);
            if calls.get() > 8 {
                Err(io::Error::other("cancelled"))
            } else {
                Ok(())
            }
        });
    assert!(result.is_err());
    assert!(calls.get() < 15);
}

#[test]
fn stopping_listener_closes_and_joins_idle_tls_workers() {
    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let cfg = server_config(&id).unwrap();
    let listener = TcpListener::bind("[::1]:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let core = Arc::new(ReceiverCore::new(ReceiverOptions::new(
        "receiver",
        "Windows",
        root.path(),
    )));
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    let (tx, rx) = mpsc::channel();
    let worker =
        thread::spawn(move || ReceiverCore::run_tcp_listener(listener, cfg, core, stop, tx));
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ReceiverEvent::Listening
    ));
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    thread::sleep(Duration::from_millis(40));
    let start = std::time::Instant::now();
    shutdown.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!matches!(socket.read(&mut [0]), Ok(1..)));
}

fn client(
    address: std::net::SocketAddrV6,
) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    client_with_identity(address, None)
}

fn client_with_identity(
    address: std::net::SocketAddrV6,
    identity: Option<&Identity>,
) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    let socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let identity = identity.map(|identity| {
        owly_airdrop::driver::tls::Identity::from_der(
            identity.certificate_der.clone(),
            identity.private_key_der.clone(),
        )
    });
    let cfg = owly_airdrop::driver::tls::client_config(identity.as_ref()).unwrap();
    let connection =
        rustls::ClientConnection::new(cfg, owly_airdrop::driver::tls::server_name()).unwrap();
    rustls::StreamOwned::new(connection, socket)
}

#[test]
fn pinned_file_offer_requires_the_same_proven_tls_certificate() {
    use owly_airdrop::proto::messages::Offer;
    use sha2::{Digest, Sha256};

    let (_source, file_ask, upload) = protocol_upload(Compressor::Gzip, false);
    let root = tempfile::tempdir().unwrap();
    let receiver = identity(&root.path().join("receiver"));
    let trusted = identity(&root.path().join("trusted"));
    let other = identity(&root.path().join("other"));
    let pin: [u8; 32] = Sha256::digest(&trusted.certificate_der).into();
    let url_ask = sender::ask_request(
        &sender::Config::new("iPhone", "abcdef123456"),
        "LINK123",
        &Offer::Urls(vec!["https://example.com".into()]),
    );

    for (client_id, ask, accepted) in [
        (None, &file_ask, false),
        (Some(&other), &file_ask, false),
        (Some(&trusted), &url_ask, false),
        (Some(&trusted), &file_ask, true),
    ] {
        let opts = ReceiverOptions::new("receiver", "Windows", root.path().join("received"));
        let core = Arc::new(
            ReceiverCore::new(opts)
                .with_decision(move |offer| offer.file_offer_certificate() == Some(pin)),
        );
        let (peer, worker) = start(core, &receiver);
        let mut tls = client_with_identity(peer.service.address, client_id);
        tls.write_all(ask).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, if accepted { 200 } else { 403 });
        if accepted {
            tls.write_all(&upload).unwrap();
            tls.flush().unwrap();
            assert_eq!(response(&mut tls).status, 200);
        }
        tls.conn.send_close_notify();
        let _ = tls.flush();
        drop(tls);
        let events = worker.join().unwrap().unwrap();
        assert_eq!(
            events
                .iter()
                .any(|e| matches!(e, ReceiverEvent::Received { .. })),
            accepted
        );
    }
}

fn response(stream: &mut impl Read) -> sender::Response {
    let mut reader = ResponseReader::new();
    let mut buf = [0; 4096];
    loop {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "missing HTTP response");
        if let Some(reply) = reader.receive(&buf[..n]).unwrap() {
            return reply;
        }
    }
}

#[test]
fn receiver_distinguishes_active_peer_disconnect_from_a_malformed_upload() {
    for (boundary, close_notify, malformed) in [
        (false, false, false),
        (false, true, false),
        (false, false, true),
        (true, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let compressor = if boundary {
            Compressor::DvZip
        } else {
            Compressor::Gzip
        };
        let (_source, offer, mut upload) = protocol_upload(compressor, false);
        let root = tempfile::tempdir().unwrap();
        let id = identity(root.path());
        let dest = root.path().join("received");
        let mut opts = ReceiverOptions::new("receiver", "Windows", &dest).with_accept_all();
        opts.allow_dvzip_archive_boundary = boundary;
        let core = Arc::new(ReceiverCore::new(opts));
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let SocketAddr::V6(addr) = listener.local_addr().unwrap() else {
            panic!()
        };
        let config = server_config(&id).unwrap();
        let reset_at_boundary_probe = boundary && !close_notify && !malformed;
        let (upload_started, started) = mpsc::channel();
        let (reset, reset_done) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut events = Vec::new();
            let result =
                ReceiverCore::serve_tls_connection_with_events(stream, config, core, |event| {
                    if reset_at_boundary_probe && matches!(event, ReceiverEvent::ModeSelected(_)) {
                        upload_started.send(()).unwrap();
                        reset_done.recv_timeout(Duration::from_secs(3)).unwrap();
                    }
                    events.push(event)
                });
            (result, events)
        });
        let mut tls = client(addr);
        tls.write_all(&offer).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, 200);
        let body = upload
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        if malformed {
            upload[body..].fill(0);
        } else {
            upload.truncate(body + 4);
        }
        tls.write_all(&upload).unwrap();
        if close_notify {
            tls.conn.send_close_notify();
        }
        tls.flush().unwrap();
        if malformed || close_notify {
            assert_eq!(response(&mut tls).status, 400);
        }
        if reset_at_boundary_probe {
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            // Abort the actual TCP stream before the receiver can peek/read again.
            socket2::SockRef::from(&tls.sock)
                .set_linger(Some(Duration::ZERO))
                .unwrap();
        }
        drop(tls);
        if reset_at_boundary_probe {
            reset.send(()).unwrap();
        }

        let (result, events) = worker.join().unwrap();
        let expected = if malformed {
            TransferFailureKind::Validation
        } else {
            TransferFailureKind::Interrupted
        };
        assert_eq!(result.expect_err(&format!("expected {expected:?} with close_notify={close_notify}, malformed={malformed}: {events:?}")).kind(), expected);
        assert!(events.iter().any(
            |event| matches!(event, ReceiverEvent::TransferFailed { kind, .. } if *kind == expected)
        ));
        assert!(!dest.exists() || fs::read_dir(dest).unwrap().next().is_none());
    }
}

#[test]
fn tls_url_offers_finish_after_acceptance_without_uploading_files() {
    use owly_airdrop::proto::messages::Offer;
    use wayline_core::airdrop::web_links;
    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let dest = root.path().join("received");
    const URL: &str = "https://b23.tv/wayline-test";
    const CASES: [(&str, &str, &[&str]); 3] = [
        (URL, URL, &[URL]),
        (
            "复制打开抖音，看看【示例】的作品：https://v.douyin.com/WaylineTest/ 04/09\r\n第二行",
            "复制打开抖音，看看【示例】的作品：https://v.douyin.com/WaylineTest/ 04/09\n第二行",
            &["https://v.douyin.com/WaylineTest/"],
        ),
        ("分享文字\r\n没有链接", "分享文字\n没有链接", &[]),
    ];
    for (item, text, links) in CASES {
        let ask = sender::ask_request(
            &sender::Config::new("iPhone", "abcdef123456"),
            "LINK123",
            &Offer::Urls(vec![item.into()]),
        );
        for accept in [true, false] {
            let core = Arc::new(
                ReceiverCore::new(ReceiverOptions::new("receiver", "Windows", &dest))
                    .with_decision(move |offer| {
                        assert!(offer.files.is_empty());
                        assert_eq!(offer.urls, [item]);
                        assert_eq!(offer.share_text.as_deref(), Some(text));
                        assert_eq!(web_links(&offer.urls), links);
                        accept
                    }),
            );
            let (peer, worker) = start(core, &id);
            let mut tls = client(peer.service.address);
            tls.write_all(&ask).unwrap();
            tls.flush().unwrap();
            assert_eq!(response(&mut tls).status, if accept { 200 } else { 403 });
            tls.conn.send_close_notify();
            let _ = tls.flush();
            drop(tls);
            let events = worker.join().unwrap().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, ReceiverEvent::Received { transfer, files }
                        if files.is_empty()
                            && transfer.share_text.as_deref() == Some(text)
                            && web_links(&transfer.urls) == links))
                    .count(),
                usize::from(accept)
            );
            assert!(!dest.exists() || fs::read_dir(&dest).unwrap().next().is_none());
        }
    }
}

#[test]
fn tls_validated_dvzip_boundary_finishes_without_zero_chunk_or_eof() {
    let (_source, offer, wire) = protocol_upload(Compressor::DvZip, true);
    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let dest = root.path().join("received");
    let mut opts = ReceiverOptions::new("receiver", "Windows", &dest).with_accept_all();
    opts.allow_dvzip_archive_boundary = true;
    let (peer, worker) = start(Arc::new(ReceiverCore::new(opts)), &id);
    let mut tls = client(peer.service.address);
    tls.write_all(&offer).unwrap();
    tls.flush().unwrap();
    assert_eq!(response(&mut tls).status, 200);
    tls.write_all(&wire).unwrap();
    tls.flush().unwrap();
    assert_eq!(response(&mut tls).status, 200);
    let events = worker.join().unwrap().unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, ReceiverEvent::ArchiveBoundary { .. })));
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"verified");
}

#[test]
fn tls_receive_publishes_both_top_level_files_from_one_offer() {
    use owly_airdrop::proto::{
        archive::{compress::Encoder, cpio},
        messages::{AskFile, Offer},
    };

    for (compressor, wrapped, same_name, omit_last_chunk) in [
        (Compressor::Gzip, false, false, false),
        (Compressor::Gzip, true, false, false),
        (Compressor::DvZip, true, false, true),
        (Compressor::DvZip, true, true, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let identity = identity(root.path());
        let image_name = "IMG_001.HEIC";
        let (second_name, second_type) = if same_name {
            (image_name, "public.heic")
        } else {
            ("视频.MOV", "com.apple.quicktime-movie")
        };
        let entry_header = |name: &str, mode: u32, size: usize| {
            cpio::odc_header(&cpio::Entry {
                name: name.into(),
                mode,
                size: size as u64,
                nlink: 1,
                ino: 1,
                uid: 501,
                gid: 20,
                mtime: 0,
            })
        };
        let mut raw_archive = entry_header(".", cpio::MODE_DIR | 0o755, 0);
        let mut files = Vec::new();
        for (name, kind, wrapper, data) in [
            (image_name, "public.heic", "./photo", b"image data"),
            (second_name, second_type, "./other", b"other data"),
        ] {
            let bom_path = if wrapped {
                raw_archive.extend(entry_header(wrapper, cpio::MODE_DIR | 0o755, 0));
                format!("{wrapper}/{name}")
            } else {
                format!("./{name}")
            };
            raw_archive.extend(entry_header(&bom_path, cpio::MODE_FILE | 0o644, data.len()));
            raw_archive.extend(data);
            files.push(AskFile {
                name: name.into(),
                file_type: kind.into(),
                size: Some(data.len() as u64),
                bom_path,
                is_directory: false,
            });
        }
        raw_archive.extend(cpio::odc_trailer());
        let mut encoder = Encoder::new(compressor, flate2::Compression::fast());
        let mut body = encoder.write(&raw_archive);
        body.extend(encoder.finish());

        let destination = root.path().join("received");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join(image_name), b"existing file").unwrap();
        let mut options = ReceiverOptions::new("receiver", "Windows", &destination);
        options.allow_dvzip_archive_boundary = true;
        let core = ReceiverCore::new(options).with_decision(move |transfer| {
            assert_eq!(
                transfer
                    .files
                    .iter()
                    .map(|file| file.name.as_str())
                    .collect::<Vec<_>>(),
                [image_name, second_name]
            );
            true
        });
        let (peer, worker) = start(Arc::new(core), &identity);
        let mut tls = client(peer.service.address);
        let ask = sender::ask_request(
            &sender::Config::new("iPhone", "abcdef123456"),
            "MULTI-FILE",
            &Offer::Files(files),
        );
        let plist = http::parse_request(&ask, &http::Limits::default())
            .unwrap()
            .unwrap()
            .1;
        tls.write_all(&http::post_head(
            "/Ask",
            &[("Transfer-Encoding", "chunked")],
        ))
        .unwrap();
        for chunk in ask[plist..].chunks(67) {
            tls.write_all(&http::chunk(chunk)).unwrap();
        }
        tls.write_all(b"0\r\n\r\n").unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, 200, "wrapped: {wrapped}");
        tls.write_all(&http::post_head(
            "/Upload",
            &[
                ("Content-Type", compressor.content_type()),
                ("TransferID", "MULTI-FILE"),
                ("Transfer-Encoding", "chunked"),
            ],
        ))
        .unwrap();
        for chunk in body.chunks(23) {
            tls.write_all(&http::chunk(chunk)).unwrap();
        }
        if !omit_last_chunk {
            tls.write_all(b"0\r\n\r\n").unwrap();
        }
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, 200);
        let events = worker.join().unwrap().unwrap();
        assert!(matches!(events.first(), Some(ReceiverEvent::AskStarted)));
        assert_eq!(
            fs::read(destination.join(image_name)).unwrap(),
            b"existing file"
        );
        let expected = [
            destination.join("IMG_001 (1).HEIC"),
            destination.join(if same_name {
                "IMG_001 (2).HEIC"
            } else {
                second_name
            }),
        ];
        assert_eq!(fs::read(&expected[0]).unwrap(), b"image data");
        assert_eq!(fs::read(&expected[1]).unwrap(), b"other data");
        assert!(
            matches!(events.last(), Some(ReceiverEvent::Received { transfer, files })
            if transfer.files.len() == 2 && files.as_slice() == expected)
        );
    }
}

#[test]
fn tls_batch_of_59_photos_with_nested_wrappers_publishes_all_files() {
    use owly_airdrop::proto::{
        archive::{compress::Encoder, cpio},
        messages::{AskFile, Offer},
    };

    const PHOTO_COUNT: u8 = 59;
    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let destination = root.path().join("received");
    let header = |name: &str, mode, size| {
        cpio::odc_header(&cpio::Entry {
            name: name.into(),
            mode,
            size,
            nlink: 1,
            ino: 1,
            uid: 501,
            gid: 20,
            mtime: 0,
        })
    };
    let mut archive = Vec::new();
    for directory in [".", "./Photos", "./Photos/Export"] {
        archive.extend(header(directory, cpio::MODE_DIR | 0o755, 0));
    }
    let mut photos = Vec::new();
    for index in 0..PHOTO_COUNT {
        let wrapper = format!("./Photos/Export/{index}");
        let name = format!("IMG_{index:03}.HEIC");
        let bom_path = format!("{wrapper}/{name}");
        // Apple can include the prefix directories or omit them.
        if index % 2 == 0 {
            archive.extend(header(&wrapper, cpio::MODE_DIR | 0o755, 0));
        }
        archive.extend(header(&bom_path, cpio::MODE_FILE | 0o644, 1));
        archive.push(index);
        photos.push(AskFile {
            name,
            file_type: "public.heic".into(),
            size: Some(1),
            bom_path,
            is_directory: false,
        });
    }
    archive.extend(cpio::odc_trailer());
    let compressor = Compressor::DvZip;
    let mut encoder = Encoder::new(compressor, flate2::Compression::fast());
    let mut upload = encoder.write(&archive);
    upload.extend(encoder.finish());

    let core = ReceiverCore::new(ReceiverOptions::new("receiver", "Windows", &destination))
        .with_decision(|offer| {
            assert_eq!(offer.files.len(), usize::from(PHOTO_COUNT));
            assert_eq!(offer.files[58].name, "IMG_058.HEIC");
            true
        });
    let (peer, worker) = start(Arc::new(core), &id);
    let mut tls = client(peer.service.address);
    let ask = sender::ask_request(
        &sender::Config::new("iPhone", "abcdef123456"),
        "PHOTO_BATCH",
        &Offer::Files(photos),
    );
    let (_, ask_body) = read_request(&mut ask.as_slice());
    tls.write_all(&http::post_head(
        "/Ask",
        &[("Transfer-Encoding", "chunked")],
    ))
    .unwrap();
    for chunk in ask_body.chunks(1024) {
        tls.write_all(&http::chunk(chunk)).unwrap();
    }
    tls.write_all(b"0\r\n\r\n").unwrap();
    tls.flush().unwrap();
    assert_eq!(response(&mut tls).status, 200);
    tls.write_all(&sender::upload_head(
        "PHOTO_BATCH",
        compressor,
        upload.len() as u64,
    ))
    .unwrap();
    tls.write_all(&upload).unwrap();
    tls.flush().unwrap();
    assert_eq!(response(&mut tls).status, 200);
    let events = worker.join().unwrap().unwrap();
    assert!(
        matches!(events.last(), Some(ReceiverEvent::Received { transfer, files })
        if transfer.files.len() == usize::from(PHOTO_COUNT) && files.len() == usize::from(PHOTO_COUNT))
    );
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        usize::from(PHOTO_COUNT)
    );
    for index in 0..PHOTO_COUNT {
        assert_eq!(
            fs::read(destination.join(format!("IMG_{index:03}.HEIC"))).unwrap(),
            [index]
        );
    }
}

#[test]
fn tls_invalid_offer_is_rejected_before_consent_with_a_diagnostic() {
    use owly_airdrop::proto::messages::{AskFile, Offer};

    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let file = |name: &str, bom_path: &str, is_directory| AskFile {
        name: name.into(),
        file_type: "public.jpeg".into(),
        size: Some(9),
        bom_path: bom_path.into(),
        is_directory,
    };
    const NAME: &str = "IMG_001.jpg";
    for (files, reason) in [
        (
            vec![file(NAME, "./photo/different-name.jpg", false)],
            "file name does not match archive path",
        ),
        (
            vec![file(NAME, "./photo/nested/../IMG_001.jpg", false)],
            "unsupported archive path component or length",
        ),
        (
            vec![file(NAME, "./photo/IMG_001.jpg", true)],
            "directory offer must name a top-level path",
        ),
        (
            vec![
                file(NAME, "./photo/nested/IMG_001.jpg", false),
                file(NAME, "photo/nested/IMG_001.jpg", false),
            ],
            "duplicate archive path in offer",
        ),
        (
            vec![
                file(NAME, "./photo/nested/IMG_001.jpg", false),
                file("nested", "./photo/nested", false),
            ],
            "file path conflicts with an offered descendant",
        ),
    ] {
        let core = ReceiverCore::new(ReceiverOptions::new("receiver", "Windows", root.path()))
            .with_decision(|_| panic!("invalid offer must not prompt"));
        let (peer, worker) = start(Arc::new(core), &id);
        let mut tls = client(peer.service.address);
        let ask = sender::ask_request(
            &sender::Config::new("iPhone", "abcdef123456"),
            "BAD-PATH",
            &Offer::Files(files),
        );
        tls.write_all(&ask).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, 400);
        assert_eq!(
            worker.join().unwrap().unwrap(),
            [
                ReceiverEvent::AskStarted,
                ReceiverEvent::Rejected {
                    path: "/Ask".into(),
                    status: 400,
                    reason: reason.into(),
                },
            ]
        );
    }
}

#[test]
fn tls_rejects_unrecognized_mixed_offers_with_counts_and_no_sender_values() {
    use owly_airdrop::proto::{
        messages::{AskFile, Offer},
        plist::{self, Dict, Value},
    };

    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let files = [
        ("private-photo.HEIC", "public.heic", 100),
        ("private-title.WEBLOC", "com.apple.web-internet-location", 0),
    ]
    .into_iter()
    .map(|(name, file_type, size)| AskFile {
        name: name.into(),
        file_type: file_type.into(),
        size: Some(size),
        bom_path: format!("./{name}"),
        is_directory: false,
    })
    .collect();
    let ask = sender::ask_request(
        &sender::Config::new("private sender", "abcdef123456"),
        "MIXED",
        &Offer::Files(files),
    );
    let (_, body_start) = http::parse_request(&ask, &http::Limits::default())
        .unwrap()
        .unwrap();
    let Value::Dict(mut offer) =
        plist::from_bytes(&ask[body_start..], &plist::Limits::default()).unwrap()
    else {
        panic!("expected an offer dictionary");
    };
    offer.insert(
        "Items".into(),
        Value::Array(vec![
            "https://example.invalid/private-url".into(),
            "private-app://path".into(),
        ]),
    );
    for (transfer_type, expected) in [
        (Some("links"), "links"),
        (Some("private-type"), "other"),
        (None, "missing"),
    ] {
        if let Some(kind) = transfer_type {
            offer.insert(
                "TransferType".into(),
                Value::Dict(Dict::from([(kind.into(), Value::Dict(Dict::new()))])),
            );
        } else {
            offer.remove("TransferType");
        }
        let body = plist::to_bytes(&Value::Dict(offer.clone()));
        let request = http::post_head("/Ask", &[("Content-Length", &body.len().to_string())]);
        let core = ReceiverCore::new(ReceiverOptions::new("receiver", "Windows", root.path()))
            .with_decision(|_| panic!("mixed offer must not prompt"));
        let (peer, worker) = start(Arc::new(core), &id);
        let mut tls = client(peer.service.address);
        tls.write_all(&request).unwrap();
        tls.write_all(&body).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, 400);
        assert_eq!(
            worker.join().unwrap().unwrap(),
            [
                ReceiverEvent::AskStarted,
                ReceiverEvent::Rejected {
                    path: "/Ask".into(),
                    status: 400,
                    reason: format!(
                        "both files and URLs offered (transfer_type={expected}, files=2, items=2, shortcut_files=1, empty_link_metadata_files=1)"
                    )
                    .into(),
                },
            ]
        );
    }
}

#[test]
fn tls_file_share_with_a_web_link_requires_consent_and_uploads_the_attachment() {
    use owly_airdrop::proto::plist::{self, Value};

    const WEB_LINKS: &[&str] = &["https://b23.tv/wayline-test"];
    for (item, decision, status, has_link) in [
        (
            "【Bilibili】https://b23.tv/wayline-test",
            Some(true),
            200,
            true,
        ),
        (WEB_LINKS[0], Some(false), 403, true),
        (WEB_LINKS[0], None, 403, true),
        (
            "bilibili://video/1 file:///C:/test.exe",
            Some(true),
            200,
            false,
        ),
    ] {
        let expected_links = if has_link { WEB_LINKS } else { &[] };
        let (root, ask, upload) = protocol_upload(Compressor::DvZip, false);
        let receiver = identity(&root.path().join("receiver"));
        let sender = identity(&root.path().join("sender"));
        let destination = root.path().join("received");
        let (_, body) = read_request(&mut ask.as_slice());
        let Value::Dict(mut offer) = plist::from_bytes(&body, &plist::Limits::default()).unwrap()
        else {
            panic!("expected an offer dictionary");
        };
        offer.insert("Items".into(), Value::Array(vec![item.into()]));
        let body = plist::to_bytes(&Value::Dict(offer));
        let head = http::post_head("/Ask", &[("Content-Length", &body.len().to_string())]);
        let options = ReceiverOptions::new("receiver", "Windows", &destination).with_accept_all();
        let mut core = ReceiverCore::new(options);
        let prompted = Arc::new(AtomicBool::new(false));
        if let Some(decision) = decision {
            let prompted = Arc::clone(&prompted);
            core = core.with_decision(move |transfer| {
                prompted.store(true, Ordering::Release);
                assert_eq!(transfer.files.len(), 1);
                assert_eq!(transfer.files[0].name, "a.txt");
                let actual = wayline_core::airdrop::web_links(&transfer.urls);
                assert_eq!(actual, expected_links);
                assert_eq!(transfer.share_text.as_deref(), Some(item));
                assert!(transfer.sender_certificate_sha256.is_some());
                assert!(transfer.file_offer_certificate().is_none());
                let mut normalized = transfer.clone();
                normalized.urls = actual;
                assert!(normalized.file_offer_certificate().is_none());
                decision
            });
        }
        let (peer, worker) = start(Arc::new(core), &receiver);
        let mut tls = client_with_identity(peer.service.address, Some(&sender));
        tls.write_all(&head).unwrap();
        tls.write_all(&body).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, status);
        assert_eq!(prompted.load(Ordering::Acquire), decision.is_some());
        assert!(!destination.exists());
        if status == 200 {
            tls.write_all(&upload).unwrap();
            tls.flush().unwrap();
            assert_eq!(response(&mut tls).status, 200);
        }
        tls.conn.send_close_notify();
        let _ = tls.flush();
        drop(tls);
        let events = worker.join().unwrap().unwrap();
        let accepted_links: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ReceiverEvent::WebLinksAccepted(transfer) => Some(transfer),
                _ => None,
            })
            .collect();
        assert_eq!(accepted_links.len(), usize::from(status == 200 && has_link));
        for transfer in accepted_links {
            assert_eq!(transfer.share_text.as_deref(), Some(item));
        }
        if status == 200 {
            let saved = destination.join("a.txt");
            assert_eq!(fs::read(&saved).unwrap(), b"verified");
            assert!(
                matches!(events.last(), Some(ReceiverEvent::Received { transfer, files })
                if files == &[saved]
                    && transfer.share_text.as_deref() == Some(item)
                    && wayline_core::airdrop::web_links(&transfer.urls) == expected_links)
            );
        } else {
            assert!(!destination.exists());
            assert!(matches!(
                events.last(),
                Some(ReceiverEvent::Rejected { .. })
            ));
        }
    }
}

#[test]
fn tls_publishes_web_links_before_a_file_upload_and_reports_the_sender_interrupting() {
    use owly_airdrop::proto::plist::{self, Value};

    let (root, ask, _) = protocol_upload(Compressor::DvZip, false);
    let id = identity(root.path());
    let destination = root.path().join("received");
    let server = server_config(&id).unwrap();
    let listener = TcpListener::bind("[::1]:0").unwrap();
    let SocketAddr::V6(address) = listener.local_addr().unwrap() else {
        panic!()
    };
    let (events, activity) = mpsc::channel();
    let core = ReceiverCore::new(ReceiverOptions::new("receiver", "Windows", &destination))
        .with_decision(|_| true);
    let worker = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        ReceiverCore::serve_tls_connection_with_events(socket, server, Arc::new(core), |event| {
            let _ = events.send(event);
        })
    });
    let (_, body) = read_request(&mut ask.as_slice());
    let Value::Dict(mut offer) = plist::from_bytes(&body, &plist::Limits::default()).unwrap()
    else {
        panic!("expected an offer dictionary");
    };
    let url = "https://b23.tv/wayline-test";
    offer.insert("Items".into(), Value::Array(vec![url.into()]));
    let body = plist::to_bytes(&Value::Dict(offer));
    let head = http::post_head("/Ask", &[("Content-Length", &body.len().to_string())]);
    let mut tls = client(address);
    tls.write_all(&head).unwrap();
    tls.write_all(&body).unwrap();
    tls.flush().unwrap();
    assert_eq!(response(&mut tls).status, 200);
    let wait = Duration::from_secs(3);
    assert_eq!(
        activity.recv_timeout(wait).unwrap(),
        ReceiverEvent::AskStarted
    );
    assert!(matches!(
        activity.recv_timeout(wait).unwrap(),
        ReceiverEvent::Asked(_)
    ));
    assert!(
        matches!(activity.recv_timeout(wait).unwrap(), ReceiverEvent::WebLinksAccepted(transfer) if transfer.urls == [url] && transfer.files.len() == 1)
    );
    assert!(!destination.exists());

    tls.conn.send_close_notify();
    tls.flush().unwrap();
    drop(tls);
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        TransferFailureKind::Interrupted
    );
    assert!(matches!(
        activity.recv_timeout(wait).unwrap(),
        ReceiverEvent::TransferFailed {
            kind: TransferFailureKind::Interrupted,
            ..
        }
    ));
    assert!(activity.try_recv().is_err());
    assert!(!destination.exists());
}

#[test]
fn tls_explicit_link_metadata_offers_require_valid_urls_and_consent() {
    use owly_airdrop::proto::plist::{self, Dict, Value};

    let root = tempfile::tempdir().unwrap();
    let receiver = identity(&root.path().join("receiver"));
    let sender = identity(&root.path().join("trusted sender"));
    let destination = root.path().join("received");
    let url = "https://b23.tv/wayline-test";
    let webloc = ("Bilibili.webloc", "com.apple.web-internet-location", 0);
    for (item, (name, file_type, file_size), decision, status, expected_prompt) in [
        (url, webloc, Some(true), 200, true),
        (
            url,
            ("Bilibili.url", "public.url", 0),
            Some(true),
            200,
            true,
        ),
        (url, webloc, Some(false), 403, true),
        (url, webloc, None, 403, false),
        ("bilibili://video/1", webloc, Some(true), 403, false),
        (
            "https://user:password@example.invalid/",
            webloc,
            Some(true),
            403,
            false,
        ),
        (
            url,
            ("Bilibili.webloc", "com.apple.web-internet-location", 42),
            Some(true),
            400,
            false,
        ),
        (
            url,
            ("attachment.txt", "public.url", 0),
            Some(true),
            400,
            false,
        ),
    ] {
        let body = plist::to_bytes(&Value::Dict(Dict::from([
            (
                "TransferType".into(),
                Value::Dict(Dict::from([("links".into(), Value::Dict(Dict::new()))])),
            ),
            ("Items".into(), Value::Array(vec![item.into()])),
            (
                "Files".into(),
                Value::Array(vec![Value::Dict(Dict::from([
                    ("FileName".into(), name.into()),
                    ("FileBomPath".into(), format!("./{name}").into()),
                    ("FileSize".into(), Value::Integer(file_size)),
                    ("FileType".into(), file_type.into()),
                ]))]),
            ),
        ])));
        let head = http::post_head("/Ask", &[("Content-Length", &body.len().to_string())]);
        let options = ReceiverOptions::new("receiver", "Windows", &destination).with_accept_all();
        let mut core = ReceiverCore::new(options);
        let prompted = Arc::new(AtomicBool::new(false));
        if let Some(decision) = decision {
            let seen = Arc::clone(&prompted);
            core = core.with_decision(move |transfer| {
                seen.store(true, Ordering::Release);
                assert!(transfer.files.is_empty());
                assert_eq!(transfer.urls, [url]);
                assert!(transfer.sender_certificate_sha256.is_some());
                assert!(transfer.file_offer_certificate().is_none());
                decision
            });
        }
        let (peer, worker) = start(Arc::new(core), &receiver);
        let mut tls = client_with_identity(peer.service.address, Some(&sender));
        tls.write_all(&head).unwrap();
        tls.write_all(&body).unwrap();
        tls.flush().unwrap();
        assert_eq!(response(&mut tls).status, status);
        tls.conn.send_close_notify();
        let _ = tls.flush();
        drop(tls);
        let events = worker.join().unwrap().unwrap();
        assert_eq!(prompted.load(Ordering::Acquire), expected_prompt);
        if status == 200 {
            assert!(
                matches!(events.last(), Some(ReceiverEvent::Received { transfer, files })
                if files.is_empty() && transfer.urls == [url])
            );
        } else {
            assert!(matches!(
                events.last(),
                Some(ReceiverEvent::Rejected { .. })
            ));
        }
        assert!(!destination.exists());
    }
}

#[test]
fn final_response_failure_rolls_back_committed_files() {
    let root = tempfile::tempdir().unwrap();
    let id = identity(root.path());
    let dest = root.path().join("received");
    let source = root.path().join("data");
    fs::write(&source, b"verified").unwrap();
    let core = Arc::new(ReceiverCore::new(
        ReceiverOptions::new("receiver", "Windows", &dest).with_accept_all(),
    ));
    let config = server_config(&id).unwrap();
    let listener = TcpListener::bind("[::1]:0").unwrap();
    let SocketAddr::V6(address) = listener.local_addr().unwrap() else {
        panic!()
    };
    let worker =
        thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let write_end = socket.try_clone().unwrap();
            let mut completed = false;
            let result =
                ReceiverCore::serve_tls_connection_with_events(socket, config, core, |event| {
                    match event {
                        ReceiverEvent::Confirming { .. } => {
                            write_end.shutdown(std::net::Shutdown::Write).unwrap()
                        }
                        ReceiverEvent::Received { .. } => completed = true,
                        _ => {}
                    }
                });
            assert!(!completed);
            assert!(result.is_err());
        });
    assert!(send_to_receiver(
        &selected(address),
        &id,
        &[source],
        &options(SendFormat::Gzip),
        |_| {}
    )
    .is_err());
    worker.join().unwrap();
    assert!(fs::read_dir(dest).unwrap().next().is_none());
}

#[test]
fn request_paths_and_unoffered_files_are_rejected_by_the_protocol() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("allowed");
    fs::write(&file, b"a").unwrap();
    let (offer, _, _) = prepare(&[file], Compressor::Gzip).unwrap();
    let mut value = match offer {
        owly_airdrop::proto::messages::Offer::Files(files) => files,
        _ => panic!(),
    };
    value[0].bom_path = "../escape".into();
    let offer = owly_airdrop::proto::messages::Offer::Files(value);
    let cfg = sender::Config::new("sender", "123456abcdef");
    let bytes = sender::ask_request(&cfg, "id", &offer);
    let mut c = Connection::new(Arc::new(Config::new("receiver")));
    c.receive(&bytes);
    assert!(!drain(&mut c).iter().any(|e| matches!(e, Event::Ask(_))));
}

fn read_request(stream: &mut impl Read) -> (http::RequestHead, Vec<u8>) {
    let mut pending = Vec::new();
    let mut buffer = [0; 4096];
    let (head, n) = loop {
        if let Some(head) = http::parse_request(&pending, &http::Limits::default()).unwrap() {
            break head;
        }
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        pending.extend_from_slice(&buffer[..n]);
    };
    pending.drain(..n);
    let mut decoder = http::BodyDecoder::new(head.body, 1 << 20, &http::Limits::default()).unwrap();
    let mut body = Vec::new();
    while !decoder.is_done() {
        let step = decoder.decode(&pending).unwrap();
        body.extend_from_slice(&pending[step.data]);
        pending.drain(..step.consumed);
        if step.consumed == 0 {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            pending.extend_from_slice(&buffer[..n]);
        }
    }
    (head, body)
}

#[test]
fn sender_reconnects_after_acceptance_and_requires_successful_final_response() {
    for status in [200, 403, 0] {
        let root = tempfile::tempdir().unwrap();
        let id = identity(root.path());
        let file = root.path().join("a.txt");
        fs::write(&file, b"verified").unwrap();
        let cfg = server_config(&id).unwrap();
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let SocketAddr::V6(addr) = listener.local_addr().unwrap() else {
            panic!()
        };
        let worker = thread::spawn(move || {
            let accept = || {
                let (s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                rustls::StreamOwned::new(rustls::ServerConnection::new(cfg.clone()).unwrap(), s)
            };
            let mut first = accept();
            let (head, _) = read_request(&mut first);
            assert_eq!(head.target, "/Ask");
            first
                .write_all(&http::response(200, &[("Connection", "close")], b""))
                .unwrap();
            first.flush().unwrap();
            drop(first);
            let mut second = accept();
            let (head, body) = read_request(&mut second);
            assert_eq!(head.target, "/Upload");
            assert!(!body.is_empty());
            if status != 0 {
                second
                    .write_all(&http::response(status, &[("Connection", "close")], b""))
                    .unwrap();
                second.flush().unwrap();
            }
        });
        let mut completed = false;
        let result = send_to_receiver(
            &selected(addr),
            &id,
            &[file],
            &options(SendFormat::Gzip),
            |e| {
                if matches!(e, SendEvent::Completed(_)) {
                    completed = true;
                }
            },
        );
        worker.join().unwrap();
        assert_eq!(completed, status == 200);
        match status {
            200 => {
                result.unwrap();
            }
            403 => assert_eq!(result.unwrap_err().kind(), TransferFailureKind::Declined),
            _ => assert_eq!(result.unwrap_err().kind(), TransferFailureKind::Connection),
        }
    }
}
