mod apple_identity;
mod compact_send;
mod live_transfer;
mod share_result;

use crate::{
    backend::{self, Event, RadioChange},
    i18n::{self, Language},
    model::{Model, Phase, Visibility, file_size},
    native::{Action, Shell},
    preview::{FileKind, MAX_PREVIEWS},
    scroll_area::{ScrollbarExt, client_area_animation_enabled},
};
use compact_send::SendLayout;
use gpui::prelude::FluentBuilder;
use gpui::*;
use share_result::ShareResult;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use wayline_core::SendCancellation;

const BLUE: u32 = 0x0a84ff;
const INK: u32 = 0xf5f5f7;
const MUTED: u32 = 0xa0a0a8;
const SURFACE: u32 = 0x27272b;
const SHARE_RESULT_DURATION: Duration = Duration::from_secs(10);
enum ExitState {
    Stopping,
    FlushingHistory(bool),
    Failed(String),
}

enum IncomingAnswer {
    Accept,
    Decline,
    AutoAccept,
}

struct HistoryDeletion {
    records: Vec<wayline_host::TransferHistoryRecord>,
    delete_files: bool,
    remember: bool,
}

pub struct Capsule {
    model: Model,
    orb: crate::orb::ConnectionOrb,
    language: Language,
    visibility: Visibility,
    shell: Option<Shell>,
    focus: FocusHandle,
    tx: mpsc::Sender<Event>,
    cancel: Option<SendCancellation>,
    scan_cancel: Option<SendCancellation>,
    workers: Vec<std::thread::JoinHandle<()>>,
    history_thread: Option<std::thread::JoinHandle<()>>,
    firmware: crate::firmware::Model,
    firmware_cancel: Option<wayline_flash::Cancellation>,
    firmware_worker: Option<std::thread::JoinHandle<()>>,
    picker_open: bool,
    name_input: Option<Entity<crate::name_input::NameInput>>,
    name_input_subscription: Option<Subscription>,
    settings_open: bool,
    send_layout: SendLayout,
    pending_send_layout: Option<SendLayout>,
    animate_send_view: bool,
    startup_prewarm: bool,
    history_open: bool,
    history: Vec<wayline_host::TransferHistoryRecord>,
    history_detail: Option<wayline_host::TransferHistoryRecord>,
    history_loaded: bool,
    history_error: Option<String>,
    history_preference_error: Option<String>,
    history_deletion: Option<HistoryDeletion>,
    history_deleting: bool,
    history_delete_files: bool,
    history_preference_pending: bool,
    history_worker: Option<crate::history::StoreWorker>,
    notification_position: crate::settings::NotificationPosition,
    notification_duration_seconds: u64,
    desktop_notifications_enabled: bool,
    notification_window: Option<WindowHandle<crate::notification::DesktopNotification>>,
    language_menu_open: bool,
    notification_position_menu_open: bool,
    notification_duration_menu_open: bool,
    settings_error: Option<String>,
    radio_change_pending: bool,
    radio_draft: Option<RadioChange>,
    name_saved: bool,
    device_name: String,
    receiver: crate::receiving::ReceiverHost,
    monitor: Option<backend::Monitor>,
    exit_state: Option<ExitState>,
    receiver_status: Result<(), String>,
    receive: crate::receiving::Model,
    share_result: Option<ShareResult>,
    download_directory: Option<PathBuf>,
    receiver_declined_until: Option<Instant>,
    incoming: Option<crate::receiving::IncomingRequest>,
    trust_incoming: bool,
    trusted_senders: Vec<wayline_host::TrustedSender>,
    apple_identity_imported: bool,
    use_apple_identity: bool,
    drop_hovered: bool,
    opening_files: Option<u64>,
    queued_files: Vec<PathBuf>,
    queued_at: Option<Instant>,
    pinned: bool,
    last_deactivate_at: Option<Instant>,
    auto_open_download_dir: bool,
    drag_activate: bool,
    drag_active: bool,
}

impl Capsule {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        files: Vec<PathBuf>,
        background: bool,
    ) -> Self {
        let detected = Language::detect();
        let (language, language_error) = match Language::load_preference() {
            Ok(Some(language)) => (language, None),
            Ok(None) => (detected, None),
            Err(error) => (detected, Some(error)),
        };
        i18n::set_current(language);
        let text = language.strings();
        let (device_name, mut settings_error) = match crate::settings::load() {
            Ok(name) => (name, None),
            Err(error) => (
                crate::settings::default_name(),
                Some(format!("{}{error:#}", text.err_device_name_load)),
            ),
        };
        if let Some(error) = language_error {
            settings_error = Some(format!("{}{error}", text.err_language_load));
        }
        let history_delete_files = match wayline_host::load_history_delete_files() {
            Ok(value) => value,
            Err(error) => {
                settings_error = Some(format!(
                    "{}{error:#}",
                    text.history_delete_setting_load_error
                ));
                false
            }
        };
        let notification_position = match crate::settings::load_notification_position() {
            Ok(position) => position,
            Err(error) => {
                settings_error = Some(format!("{}{error:#}", text.err_notification_position_load));
                crate::settings::NotificationPosition::DEFAULT
            }
        };
        let notification_duration_seconds =
            match crate::settings::load_notification_duration_seconds() {
                Ok(seconds) => seconds,
                Err(error) => {
                    settings_error =
                        Some(format!("{}{error:#}", text.err_notification_duration_load));
                    crate::settings::DEFAULT_NOTIFICATION_DURATION_SECONDS
                }
            };
        let desktop_notifications_enabled =
            match crate::settings::load_desktop_notifications_enabled() {
                Ok(enabled) => enabled,
                Err(error) => {
                    settings_error =
                        Some(format!("{}{error:#}", text.err_notification_enabled_load));
                    true
                }
            };
        let download_directory = match crate::receiving::save_directory() {
            Ok(directory) => Some(directory),
            Err(error) => {
                settings_error = Some(format!("{}{error:#}", text.err_download_dir_load));
                None
            }
        };
        let mut report_preference_error = |error: anyhow::Error, label| {
            let message = format!("{}{label}: {error:#}", text.err_behavior_setting_load);
            if let Some(previous) = &mut settings_error {
                previous.push('\n');
                previous.push_str(&message);
            } else {
                settings_error = Some(message);
            }
        };
        let mut load_preference = |result: anyhow::Result<bool>, default, label| {
            result.unwrap_or_else(|error| {
                report_preference_error(error, label);
                default
            })
        };
        let auto_open_download_dir = load_preference(
            crate::settings::load_auto_open_download_dir(),
            crate::settings::DEFAULT_AUTO_OPEN_DOWNLOAD_DIR,
            text.auto_open_download_dir,
        );
        let pinned = load_preference(
            crate::settings::load_pinned(),
            crate::settings::DEFAULT_PINNED,
            text.pin_window,
        );
        let drag_activate = load_preference(
            crate::settings::load_drag_activate(),
            crate::settings::DEFAULT_DRAG_ACTIVATE,
            text.drag_activate,
        );
        let (compact, dark) = wayline_host::load_send_appearance().unwrap_or_else(|error| {
            report_preference_error(error, text.send_layout_title);
            (false, false)
        });
        let send_layout = match (compact, dark) {
            (false, _) => SendLayout::Classic,
            (true, false) => SendLayout::NekoLight,
            (true, true) => SendLayout::NekoDark,
        };
        let trusted_senders = wayline_host::load_trusted_senders().unwrap_or_else(|error| {
            settings_error = Some(format!("{} {error:#}", text.trusted_senders_error));
            Vec::new()
        });
        let (tx, rx) = mpsc::channel();
        let (history_worker, history_thread) = crate::history::StoreWorker::start(tx.clone());
        let (action_tx, actions) = mpsc::channel();
        if !files.is_empty() {
            let _ = action_tx.send(Action::Files(files));
        } else if !background {
            let _ = action_tx.send(Action::Show);
        }
        let shell = Shell::new(window, action_tx).expect("initialize Windows shell integration");
        let shell = Some(shell);
        if let Some(shell) = &shell {
            shell.place(false, false, false);
        }
        let receiver = crate::receiving::ReceiverHost::new(tx.clone());
        let monitor = backend::monitor(tx.clone());
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                if this.upgrade().is_none() {
                    break;
                }
                let events: Vec<_> = rx.try_iter().collect();
                let actions: Vec<_> = actions.try_iter().collect();
                if this
                    .update_in(cx, |this, window, cx| {
                        this.poll(events, actions, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            model: Model::default(),
            orb: crate::orb::ConnectionOrb::default(),
            language,
            visibility: Visibility::Hidden,
            shell,
            focus: cx.focus_handle(),
            tx,
            cancel: None,
            scan_cancel: None,
            workers: Vec::new(),
            history_thread: Some(history_thread),
            firmware: crate::firmware::Model::default(),
            firmware_cancel: None,
            firmware_worker: None,
            picker_open: false,
            name_input: None,
            name_input_subscription: None,
            settings_open: false,
            send_layout,
            pending_send_layout: None,
            animate_send_view: client_area_animation_enabled(),
            startup_prewarm: crate::integration::startup_prewarm_enabled(),
            history_open: false,
            history: Vec::new(),
            history_detail: None,
            history_loaded: false,
            history_error: None,
            history_preference_error: None,
            history_deletion: None,
            history_deleting: false,
            history_delete_files,
            history_preference_pending: false,
            history_worker: Some(history_worker),
            notification_position,
            notification_duration_seconds,
            desktop_notifications_enabled,
            notification_window: None,
            language_menu_open: false,
            notification_position_menu_open: false,
            notification_duration_menu_open: false,
            settings_error,
            radio_change_pending: false,
            radio_draft: None,
            name_saved: false,
            device_name,
            receiver,
            monitor: Some(monitor),
            exit_state: None,
            receiver_status: Err(text.receiver_starting.into()),
            receive: crate::receiving::Model::default(),
            share_result: None,
            download_directory,
            receiver_declined_until: None,
            incoming: None,
            trust_incoming: false,
            trusted_senders,
            apple_identity_imported: wayline_host::has_apple_airdrop_identity().unwrap_or(false),
            use_apple_identity: false,
            drop_hovered: false,
            opening_files: None,
            queued_files: Vec::new(),
            queued_at: None,
            pinned,
            last_deactivate_at: None,
            auto_open_download_dir,
            drag_activate,
            drag_active: false,
        }
    }

    fn poll(
        &mut self,
        events: Vec<Event>,
        actions: Vec<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exit_state.is_some() {
            // No late status, picker or discovery callback may restart work
            // while the receiver and radio service are being shut down.
            let mut shutdown_result = None;
            for event in events {
                let (event, event_at, event_unix_ms) = event.with_timestamp();
                match event {
                    Event::ShutdownDone(Ok(background_running)) => {
                        shutdown_result = Some(background_running);
                        continue;
                    }
                    Event::HistoryFlushed => {
                        let background_running =
                            matches!(self.exit_state, Some(ExitState::FlushingHistory(true)));
                        if let Some(shell) = &self.shell
                            && let Some(error) = &self.history_error
                        {
                            shell.notify(self.language.strings().history_title, error);
                        } else if background_running && let Some(shell) = &self.shell {
                            let text = self.language.strings();
                            shell.notify(text.app_exited_title, text.app_exited_detail);
                        }
                        cx.quit();
                        return;
                    }
                    Event::SendDone(id, result) => {
                        if let Some(record) =
                            self.model
                                .take_history_record(id, &result, event_at, event_unix_ms)
                            && let Some(worker) = &self.history_worker
                        {
                            worker.append(record, Vec::new());
                        }
                        let _ = self.model.apply_at(
                            Event::SendDone(id, result),
                            event_at,
                            event_unix_ms,
                        );
                    }
                    Event::Transfer(id, event) => {
                        let _ = self.model.apply_at(
                            Event::Transfer(id, event),
                            event_at,
                            event_unix_ms,
                        );
                    }
                    Event::ReceiverActivity(event, _) => {
                        let record =
                            self.receive
                                .history_record_for_event(&event, event_at, event_unix_ms);
                        let received_files = match &event {
                            wayline_core::ReceiverEvent::Received { files, .. } => files.clone(),
                            _ => Vec::new(),
                        };
                        if self.receive.apply_at(&event, event_at, event_unix_ms)
                            && let Some(record) = record
                            && let Some(worker) = &self.history_worker
                        {
                            worker.append(record, received_files.clone());
                        }
                    }
                    Event::HistoryUpdated(Err(error)) => {
                        self.history_error = Some(format!(
                            "{}{error}",
                            self.language.strings().history_save_error
                        ));
                    }
                    Event::HistoryUpdated(Ok(_)) => self.history_error = None,
                    Event::HistoryDeleted(Err(error)) => {
                        self.history_error = Some(format!(
                            "{}{error}",
                            self.language.strings().history_delete_error
                        ));
                    }
                    Event::HistoryDeleted(Ok(_)) => self.history_error = None,
                    Event::HistoryDeletePreferenceSaved(Err(error)) => {
                        self.history_preference_error = Some(format!(
                            "{}{error}",
                            self.language.strings().history_delete_setting_error
                        ));
                    }
                    Event::ShutdownDone(Err(error)) => {
                        self.exit_state = Some(ExitState::Failed(error));
                        self.expand(true, window, cx);
                        cx.notify();
                    }
                    Event::Incoming(request) => {
                        let _ = request.decision.send(false);
                    }
                    _ => {}
                }
            }
            if let Some(background_running) = shutdown_result {
                self.flush_history_and_quit(background_running);
            }
            for action in actions {
                match action {
                    Action::Quit => self.quit(window, cx),
                    Action::Show | Action::Toggle => self.expand(true, window, cx),
                    Action::Reposition => self.place(),
                    Action::RestoreTray => {
                        if let Some(shell) = self.shell.as_mut() {
                            shell.restore_tray();
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        let mut changed = false;
        self.workers.retain(|worker| !worker.is_finished());
        for event in events {
            let (event, event_at, event_unix_ms) = event.with_timestamp();
            match event {
                Event::ShellError(error) => {
                    self.model.error = Some(error);
                    changed = true;
                    continue;
                }
                Event::Firmware(event) => {
                    if self.firmware.apply(event) {
                        self.firmware_worker.take();
                        self.firmware_cancel = None;
                        self.receiver = crate::receiving::ReceiverHost::new(self.tx.clone());
                        self.receiver.configure(
                            self.model
                                .connection_ready()
                                .then(|| self.device_name.clone()),
                        );
                    }
                    changed = true;
                    continue;
                }
                Event::HistoryLoaded(result) => {
                    self.history_loaded = true;
                    match result {
                        Ok(records) => {
                            self.history = records;
                            self.history_error = None;
                        }
                        Err(error) => {
                            self.history_error = Some(format!(
                                "{}{error}",
                                self.language.strings().history_load_error
                            ));
                        }
                    }
                    changed = true;
                    continue;
                }
                Event::HistoryUpdated(result) => {
                    match result {
                        Ok(records) => {
                            self.history = records;
                            self.history_error = None;
                        }
                        Err(error) => {
                            self.history_error = Some(format!(
                                "{}{error}",
                                self.language.strings().history_save_error
                            ));
                        }
                    }
                    changed = true;
                    continue;
                }
                Event::HistoryDeleted(result) => {
                    match result {
                        Ok(records) => {
                            self.history = records;
                            self.history_error = None;
                            self.history_deletion = None;
                            self.history_detail = None;
                        }
                        Err(error) => {
                            self.history_error = Some(format!(
                                "{}{error}",
                                self.language.strings().history_delete_error
                            ));
                        }
                    }
                    self.history_deleting = false;
                    changed = true;
                    continue;
                }
                Event::HistoryDeletePreferenceSaved(result) => {
                    self.history_preference_pending = false;
                    match result {
                        Ok(value) => {
                            self.history_delete_files = value;
                            self.history_preference_error = None;
                        }
                        Err(error) => {
                            let message = format!(
                                "{}{error}",
                                self.language.strings().history_delete_setting_error
                            );
                            self.history_preference_error = Some(message);
                        }
                    }
                    changed = true;
                    continue;
                }
                Event::ReceiverStatus(status) => {
                    if let Err(error) = &status {
                        self.receive.interrupt_with_details(
                            self.language.strings().receive_interrupted.into(),
                            error.clone(),
                        );
                    }
                    self.receiver_status = status;
                    changed = true;
                    continue;
                }
                Event::DownloadDirectorySaved(result) => {
                    self.picker_open = false;
                    match result {
                        Ok(directory) => {
                            let changed_directory =
                                self.download_directory.as_ref() != Some(&directory);
                            self.download_directory = Some(directory);
                            self.settings_error = None;
                            if changed_directory {
                                self.receiver_status =
                                    Err(self.language.strings().receiver_updating_dir.into());
                                self.receiver.reload_directory();
                            }
                        }
                        Err(error) => {
                            self.settings_error = Some(format!(
                                "{}{error}",
                                self.language.strings().err_download_dir_save
                            ))
                        }
                    }
                    changed = true;
                    continue;
                }
                Event::AppleIdentityUpdated(result) => {
                    self.apple_identity_updated(result);
                    changed = true;
                    continue;
                }
                Event::RadioChanged(result) => {
                    self.radio_change_pending = false;
                    self.receiver = crate::receiving::ReceiverHost::new(self.tx.clone());
                    match result {
                        Ok(status) => {
                            self.radio_draft = None;
                            self.settings_error = None;
                            self.model.apply_at(
                                Event::Status(Ok(status.into())),
                                event_at,
                                event_unix_ms,
                            );
                        }
                        Err(error) => {
                            let text = self.language.strings();
                            self.settings_error = Some(format!("{}{error}", text.err_radio_save));
                        }
                    }
                    self.receiver.configure(
                        self.model
                            .connection_ready()
                            .then(|| self.device_name.clone()),
                    );
                    changed = true;
                    continue;
                }
                Event::Incoming(request) => {
                    let reason = if !request.active() {
                        Some("expired or receiver stopping")
                    } else if self.download_directory.as_ref() != Some(&request.save_dir) {
                        Some("download directory changed")
                    } else if self.incoming.is_some() || self.receive.active.is_some() {
                        Some("another incoming transfer is active")
                    } else if self.radio_change_pending {
                        Some("wireless settings are changing")
                    } else if self.firmware.busy || self.model.phase.busy() {
                        Some("firmware update or outgoing transfer is active")
                    } else if self.picker_open {
                        Some("file picker is open")
                    } else if self.name_input.is_some() {
                        Some("device name is being edited")
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        crate::receiving::log(&format!(
                            "incoming ask declined by desktop: {reason}"
                        ));
                        let _ = request.decision.send(false);
                    } else {
                        let trusted = crate::receiving::can_auto_accept(
                            &request.transfer,
                            &self.trusted_senders,
                        );
                        let has_links = !request.transfer.urls.is_empty();
                        self.receive.dismiss_notice();
                        self.incoming = Some(request);
                        self.trust_incoming = false;
                        self.receiver_declined_until = None;
                        if trusted {
                            crate::receiving::log(if has_links {
                                "incoming links approved by matching certificate with link permission"
                            } else {
                                "incoming files approved by matching saved certificate"
                            });
                            self.answer_incoming(IncomingAnswer::AutoAccept, cx);
                        } else {
                            self.expand(true, window, cx);
                        }
                    }
                    changed = true;
                    continue;
                }
                Event::ReceiverActivity(event, image) => {
                    if let wayline_core::ReceiverEvent::WebLinksAccepted(transfer) = &event {
                        let urls = self
                            .receive
                            .links_after_acceptance(transfer, &self.trusted_senders);
                        changed |= !urls.is_empty();
                        if !urls.is_empty() {
                            let original = transfer
                                .share_text
                                .clone()
                                .unwrap_or_else(|| urls.join("\n"));
                            self.show_share_result(
                                self.language.strings().receive_web_opened.into(),
                                transfer.sender_name.clone(),
                                Some(original),
                                image,
                                window,
                            );
                        }
                        Self::open_received_web_links(urls, cx);
                        continue;
                    }
                    if let wayline_core::ReceiverEvent::ModeSelected(_) = &event {
                        let _ = self.receive.apply_at(&event, event_at, event_unix_ms);
                        changed = true;
                        continue;
                    }
                    let record =
                        self.receive
                            .history_record_for_event(&event, event_at, event_unix_ms);
                    let received_files = match &event {
                        wayline_core::ReceiverEvent::Received { files, .. } => files.clone(),
                        _ => Vec::new(),
                    };
                    let web_links = self
                        .receive
                        .links_on_completion(&event, &self.trusted_senders);
                    let opened_early = self
                        .receive
                        .active
                        .as_ref()
                        .is_some_and(|active| active.web_links_opened);
                    let receive_start = self.receive.starting_receive(&event).map(|active| {
                        let count = self.language.file_count(active.files);
                        format!("{} · {count}", active.sender)
                    });
                    let applied = self.receive.apply_at(&event, event_at, event_unix_ms);
                    if applied && let Some(message) = receive_start {
                        self.show_desktop_notification(
                            self.language.strings().receiving_title,
                            message.clone(),
                            message,
                            crate::settings::NotificationPosition::BottomRight,
                            cx,
                        );
                    }
                    let open_location = crate::receiving::auto_open_location(
                        &event,
                        applied,
                        self.auto_open_download_dir,
                        self.download_directory.as_deref(),
                    );
                    if applied
                        && let Some(record) = record
                        && let Some(worker) = &self.history_worker
                    {
                        worker.append(record, received_files);
                    }
                    if applied && let Some(notice) = &self.receive.notice {
                        match &event {
                            wayline_core::ReceiverEvent::Received {
                                transfer, files, ..
                            } => {
                                let text = self.language.strings();
                                let title = if !files.is_empty() {
                                    text.notice_files_received
                                } else if wayline_core::airdrop::web_links(&transfer.urls)
                                    .is_empty()
                                {
                                    text.receive_shared_text
                                } else {
                                    self.language.shared_links()
                                };
                                self.show_share_result(
                                    title.into(),
                                    notice.message.clone(),
                                    transfer.share_text.clone(),
                                    image,
                                    window,
                                );
                                self.receive.notice = None;
                            }
                            wayline_core::ReceiverEvent::TransferFailed {
                                kind: wayline_core::TransferFailureKind::Connection,
                                ..
                            } if !opened_early => self.show_desktop_notification(
                                "Wayline",
                                notice.message.clone(),
                                notice.full_text.clone(),
                                self.notification_position,
                                cx,
                            ),
                            _ => {}
                        }
                    }
                    if let Some(target) = open_location {
                        self.open_history_folder(target, cx);
                    }
                    if applied {
                        Self::open_received_web_links(web_links, cx);
                    }
                    changed |= applied;
                    continue;
                }
                Event::Status(ref status) => {
                    let ready = status.as_ref().is_ok_and(backend::LinkStatus::ready);
                    // Polling continues while the sharing card is hidden; the
                    // tray must reflect the same readiness as the status label.
                    if let Some(shell) = self.shell.as_mut() {
                        shell.set_connected(ready);
                    }
                    if !self.radio_change_pending {
                        self.receiver.configure(
                            (ready && !self.firmware.busy).then(|| self.device_name.clone()),
                        );
                    }
                    if !ready {
                        changed |= self
                            .receive
                            .interrupt(self.language.strings().connection_lost_receive.into());
                    }
                }
                _ => {}
            }
            let files_ready = matches!(&event, Event::FilesValidated(id, _, Ok(_)) if *id == self.model.selection_id);
            if files_ready {
                self.clear_previews(window);
            }
            let send_record = if let Event::SendDone(id, result) = &event {
                self.model
                    .take_history_record(*id, result, event_at, event_unix_ms)
            } else {
                None
            };
            changed |= self.model.apply_at(event, event_at, event_unix_ms);
            if let Some(record) = send_record
                && let Some(worker) = &self.history_worker
            {
                worker.append(record, Vec::new());
            }
            let reconnected = std::mem::take(&mut self.model.rescan_requested);
            if files_ready || reconnected {
                self.scan(cx);
            }
        }
        for action in actions {
            match action {
                Action::Toggle => {
                    let just_deactivated = self
                        .last_deactivate_at
                        .take()
                        .is_some_and(|at| at.elapsed() < Duration::from_millis(350));
                    if !just_deactivated {
                        self.expand(self.visibility != Visibility::Share, window, cx);
                    }
                }
                Action::Show => self.expand(true, window, cx),
                Action::Close => self.expand(false, window, cx),
                Action::Deactivate => {
                    let interacting = self.picker_open
                        || self.share_result.is_some()
                        || self.incoming.is_some()
                        || self.firmware.busy
                        || self.drag_active
                        || self.drop_hovered;
                    if self.visibility.hide_on_deactivate(self.pinned, interacting) {
                        self.last_deactivate_at = Some(Instant::now());
                        self.expand(false, window, cx);
                    }
                }
                Action::Files(paths) => {
                    self.queued_files.extend(paths);
                    self.queued_at = Some(Instant::now());
                    self.expand(true, window, cx);
                }
                Action::FilesDropped(paths) => self.accept_files(paths, window, cx),
                Action::DropHover(active) => {
                    self.drop_hovered = active;
                    changed = true;
                }
                Action::FilesPicked(result) => {
                    self.picker_open = false;
                    match result {
                        Ok(Some(paths)) => self.accept_files(paths, window, cx),
                        Err(error) => {
                            self.model.error = Some(format!(
                                "{}{error}",
                                self.language.strings().err_file_picker
                            ))
                        }
                        Ok(None) => {}
                    }
                    changed = true;
                }
                Action::FilesOpened(id, result) => {
                    if self.opening_files == Some(id) {
                        self.opening_files = None;
                        if id == self.model.selection_id
                            && self.model.phase == Phase::Ready
                            && let Err(error) = result
                        {
                            self.model.error = Some(error);
                        }
                        changed = true;
                    }
                }
                Action::HistoryFolderOpened(result) => {
                    if let Err(error) = result {
                        self.history_error = Some(format!(
                            "{}{error}",
                            self.language.strings().history_open_error
                        ));
                    }
                    changed = true;
                }
                Action::HistoryFileOpened(result) => {
                    if let Err(error) = result {
                        self.history_error =
                            Some(format!("{}{error}", self.language.strings().err_open_file));
                    }
                    changed = true;
                }
                Action::DownloadDirectoryChosen(result) => {
                    match result {
                        Ok(Some(directory)) => {
                            let events = self.tx.clone();
                            self.workers.push(std::thread::spawn(move || {
                                let result = wayline_host::save_download_dir(&directory)
                                    .map_err(|error| format!("{error:#}"));
                                let _ = events.send(Event::DownloadDirectorySaved(result));
                            }));
                        }
                        Ok(None) => self.picker_open = false,
                        Err(error) => {
                            self.picker_open = false;
                            self.settings_error = Some(error);
                        }
                    }
                    changed = true;
                }
                Action::AppleIdentityDirectoryChosen(result) => {
                    self.apple_identity_directory_chosen(result);
                    changed = true;
                }
                Action::FileDrag(active) => {
                    self.drag_active = active;
                    if !self.model.phase.busy() && !self.picker_open && self.name_input.is_none()
                        || !active
                    {
                        self.visibility.file_drag(active, self.drag_activate);
                        self.place();
                        changed = true;
                    }
                }
                Action::ShellError(error) => {
                    self.model.error = Some(error);
                    changed = true;
                }
                Action::Choose => self.choose(window, cx),
                Action::Quit => {
                    self.quit(window, cx);
                    return;
                }
                Action::Reposition => {
                    let animate = client_area_animation_enabled();
                    changed |= self.animate_send_view != animate;
                    self.animate_send_view = animate;
                    self.place();
                }
                Action::RestoreTray => {
                    if let Some(shell) = self.shell.as_mut() {
                        shell.restore_tray();
                    }
                }
            }
        }
        if self
            .incoming
            .as_ref()
            .is_some_and(|request| !request.active())
        {
            self.answer_incoming(IncomingAnswer::Decline, cx);
            self.receive.show_notice(
                self.language.strings().incoming_ended,
                crate::receiving::NoticeKind::Info,
            );
            changed = true;
        }
        let now = Instant::now();
        if self
            .share_result
            .as_ref()
            .is_some_and(|result| now >= result.deadline)
        {
            self.finish_share_result(window, cx);
        }
        if self
            .model
            .send_completion_expired(now, SHARE_RESULT_DURATION)
            && self.queued_at.is_none()
        {
            self.reset(window, cx);
        }
        changed |= self.receive.expire_notice(now);
        changed |= self.model.expire_speed(now);
        changed |= self.receive.expire_speed(now);
        changed |= self.model.phase.busy() && self.model.send_elapsed(now).is_some();
        changed |= self.receive.active.is_some() && self.receive.receive_elapsed(now).is_some();
        if self
            .receiver_declined_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.receiver_declined_until = None;
            changed = true;
        }
        // Static Explorer verbs can launch once per selected file; coalesce the
        // batch before doing filesystem metadata work or device discovery.
        if self
            .queued_at
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(350))
        {
            self.queued_at = None;
            let paths = std::mem::take(&mut self.queued_files);
            self.accept_files(paths, window, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_quit(false, window, cx);
    }

    fn begin_quit(
        &mut self,
        firmware_confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.exit_state,
            Some(ExitState::Stopping | ExitState::FlushingHistory(_))
        ) {
            return;
        }
        if !self.firmware.request_exit(firmware_confirmed) {
            self.expand(true, window, cx);
            self.firmware.open = true;
            cx.notify();
            return;
        }
        if let Some(cancel) = &self.firmware_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &self.scan_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if let Some(request) = self.incoming.take() {
            let _ = request.decision.send(false);
        }
        self.exit_state = Some(ExitState::Stopping);
        if let Some(shell) = self.shell.as_mut() {
            shell.set_connected(false);
        }
        self.finish_name_edit(window);
        self.settings_open = false;
        self.history_open = false;
        self.language_menu_open = false;
        self.expand(true, window, cx);
        let mut workers = std::mem::take(&mut self.workers);
        if let Some(worker) = self.firmware_worker.take() {
            workers.push(worker);
        }
        backend::shutdown(
            self.monitor.take(),
            self.receiver.shutdown(),
            workers,
            self.tx.clone(),
        );
        cx.notify();
    }

    fn flush_history_and_quit(&mut self, background_running: bool) {
        let Some(history) = self.history_worker.take() else {
            self.exit_state = Some(ExitState::FlushingHistory(background_running));
            let _ = self.tx.send(Event::HistoryFlushed);
            return;
        };
        self.exit_state = Some(ExitState::FlushingHistory(background_running));
        let flush = history.flush_handle();
        drop(history);
        let history_thread = self.history_thread.take();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = flush.join();
            if let Some(worker) = history_thread {
                let _ = worker.join();
            }
            let _ = tx.send(Event::HistoryFlushed);
        });
    }
    fn place(&self) {
        if let Some(shell) = &self.shell {
            shell.place(
                self.visibility == Visibility::Share,
                self.visibility != Visibility::Hidden,
                false,
            );
        }
    }
    fn toggle_auto_open_download_dir(&mut self, cx: &mut Context<Self>) {
        let result = crate::settings::toggle(
            &mut self.auto_open_download_dir,
            crate::settings::save_auto_open_download_dir,
        );
        let text = self.language.strings();
        self.settings_error = result.err().map(|error| {
            format!(
                "{}{}: {error:#}",
                text.err_behavior_setting_save, text.auto_open_download_dir
            )
        });
        cx.notify();
    }

    fn toggle_pinned(&mut self, cx: &mut Context<Self>) {
        let result = crate::settings::toggle(&mut self.pinned, crate::settings::save_pinned);
        let text = self.language.strings();
        self.settings_error = result.err().map(|error| {
            format!(
                "{}{}: {error:#}",
                text.err_behavior_setting_save, text.pin_window
            )
        });
        cx.notify();
    }

    fn toggle_drag_activate(&mut self, cx: &mut Context<Self>) {
        let result =
            crate::settings::toggle(&mut self.drag_activate, crate::settings::save_drag_activate);
        let text = self.language.strings();
        self.settings_error = result.err().map(|error| {
            format!(
                "{}{}: {error:#}",
                text.err_behavior_setting_save, text.drag_activate
            )
        });
        cx.notify();
    }

    fn set_send_layout(&mut self, layout: SendLayout, cx: &mut Context<Self>) {
        if layout != self.send_layout {
            self.pending_send_layout = Some(layout);
            cx.notify();
        }
    }

    fn refresh_send_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.pending_send_layout.take() else {
            return;
        };
        self.settings_error = match wayline_host::save_send_appearance(
            layout != SendLayout::Classic,
            layout == SendLayout::NekoDark,
        ) {
            Ok(()) => {
                self.send_layout = layout;
                self.back(window, cx);
                None
            }
            Err(error) => {
                let text = self.language.strings();
                Some(format!(
                    "{}{}: {error:#}",
                    text.err_behavior_setting_save, text.send_layout_title
                ))
            }
        };
        cx.notify();
    }

    fn dismiss_send_layout_refresh(&mut self, cx: &mut Context<Self>) {
        self.pending_send_layout = None;
        cx.notify();
    }

    fn toggle_startup_prewarm(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.startup_prewarm;
        self.settings_error = match crate::integration::set_startup_prewarm(enabled) {
            Ok(()) => {
                self.startup_prewarm = enabled;
                None
            }
            Err(error) => {
                let text = self.language.strings();
                Some(format!(
                    "{}{}: {error:#}",
                    text.err_behavior_setting_save, text.startup_prewarm
                ))
            }
        };
        cx.notify();
    }

    fn expand(&mut self, expanded: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !expanded {
            self.clear_share_result(window);
            self.receive.reset_notice_hover();
            self.answer_incoming(IncomingAnswer::Decline, cx);
        }
        self.drop_hovered = false;
        self.settings_open = false;
        self.pending_send_layout = None;
        self.history_open = false;
        if !self.history_deleting {
            self.history_deletion = None;
        }
        self.language_menu_open = false;
        self.finish_name_edit(window);
        self.name_saved = false;
        self.visibility = if expanded {
            Visibility::Share
        } else {
            Visibility::Hidden
        };
        if let Some(shell) = &self.shell {
            shell.place(expanded, expanded, expanded);
        }
        if expanded {
            self.focus.focus(window);
        }
        cx.notify();
    }
    fn can_open_files(&self) -> bool {
        self.model.phase == Phase::Ready
            && !self.model.files.is_empty()
            && !self.model.validating
            && !self.picker_open
            && self.name_input.is_none()
            && self.queued_at.is_none()
            && self.opening_files.is_none()
    }
    fn open_files(&mut self, cx: &mut Context<Self>) {
        if !self.can_open_files() {
            return;
        }
        if let Some(shell) = &self.shell {
            match shell.open_files(self.model.selection_id, self.model.files.clone()) {
                Ok(()) => {
                    self.opening_files = Some(self.model.selection_id);
                    self.model.error = None;
                }
                Err(error) => {
                    self.model.error =
                        Some(format!("{}{error}", self.language.strings().err_open_file))
                }
            }
            cx.notify();
        }
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.firmware.busy
            || self.model.phase.busy()
            || self.picker_open
            || self.name_input.is_some()
            || self.incoming.is_some()
        {
            return;
        }
        self.expand(true, window, cx);
        self.picker_open = true;
        if let Some(shell) = &self.shell
            && let Err(error) = shell.choose_files()
        {
            self.picker_open = false;
            self.model.error = Some(format!(
                "{}{error}",
                self.language.strings().err_file_picker
            ));
        }
        cx.notify();
    }
    fn edit_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.firmware.busy
            || self.model.phase.busy()
            || self.picker_open
            || self.name_input.is_some()
            || self.incoming.is_some()
            || self.receive.active.is_some()
        {
            return;
        }
        let input = cx.new(|cx| crate::name_input::NameInput::new(self.device_name.clone(), cx));
        self.name_input_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                    crate::name_input::NameInputEvent::Changed => {
                        this.settings_error = None;
                        this.name_saved = false;
                        cx.notify();
                    }
                    crate::name_input::NameInputEvent::Submit => this.save_name(window, cx),
                    crate::name_input::NameInputEvent::Cancel => this.cancel_name(window, cx),
                }),
            );
        input.focus_handle(cx).focus(window);
        self.name_input = Some(input);
        self.settings_error = None;
        self.name_saved = false;
        cx.notify();
    }
    fn can_change_download_directory(&self) -> bool {
        !self.firmware.busy
            && !self.picker_open
            && self.name_input.is_none()
            && self.incoming.is_none()
            && self.receive.active.is_none()
            && !self.receiver.has_active_transfers()
    }

    fn radio_status(&self) -> Option<&backend::LinkStatus> {
        self.model
            .status
            .as_ref()
            .and_then(|status| status.as_ref().ok())
    }

    fn can_edit_radio(&self) -> bool {
        !self.radio_change_pending
            && !self.model.phase.busy()
            && !self.model.scanning
            && self.can_change_download_directory()
            && self.exit_state.is_none()
    }

    fn radio_selection(&self) -> Option<RadioChange> {
        self.radio_draft
            .or_else(|| self.radio_status().map(RadioChange::from))
    }

    fn set_radio_draft(&mut self, change: RadioChange, cx: &mut Context<Self>) {
        if self.can_edit_radio() {
            self.radio_draft = Some(change);
            self.settings_error = None;
            cx.notify();
        }
    }

    fn select_radio_channel(&mut self, channel: u16, cx: &mut Context<Self>) {
        if let Some(draft) = self.radio_selection() {
            let width_mhz = draft
                .width_mhz
                .map(|width| if channel == 6 { 20 } else { width });
            self.set_radio_draft(RadioChange { channel, width_mhz }, cx);
        }
    }

    fn select_radio_width(&mut self, width: u16, cx: &mut Context<Self>) {
        if let Some(draft) = self.radio_selection()
            && draft.width_mhz.is_some()
            && (width == 20 || matches!(draft.channel, 44 | 149))
        {
            self.set_radio_draft(
                RadioChange {
                    width_mhz: Some(width),
                    ..draft
                },
                cx,
            );
        }
    }

    fn can_save_radio(&self, change: RadioChange) -> bool {
        self.can_edit_radio()
            && self.radio_status().is_some_and(|status| {
                let current = RadioChange::from(status);
                let valid_channel = match change.channel {
                    6 => change.width_mhz.unwrap_or(20) == 20,
                    44 | 149 => {
                        (current.width_mhz.is_some() && change.width_mhz == Some(20))
                            || (status.ready()
                                && matches!(
                                    status.target.as_deref(),
                                    Some("esp32c5" | "rtl8821cu")
                                ))
                    }
                    _ => false,
                };
                valid_channel
                    && current != change
                    && change.width_mhz.is_some() == current.width_mhz.is_some()
            })
    }

    fn save_radio(&mut self, cx: &mut Context<Self>) {
        let Some(change) = self
            .radio_draft
            .filter(|change| self.can_save_radio(*change))
        else {
            return;
        };
        self.radio_change_pending = true;
        self.settings_error = None;
        self.receiver_status = Err(self.language.strings().switching_radio.into());
        let receiver_worker = self.receiver.shutdown();
        self.workers.push(backend::change_radio(
            change,
            receiver_worker,
            self.tx.clone(),
        ));
        cx.notify();
    }

    fn choose_download_directory(&mut self, cx: &mut Context<Self>) {
        if !self.can_change_download_directory() {
            return;
        }
        if let Some(shell) = &self.shell {
            match shell.choose_download_directory() {
                Ok(()) => {
                    self.picker_open = true;
                    self.settings_error = None;
                }
                Err(error) => {
                    self.settings_error = Some(format!(
                        "{}{error:#}",
                        self.language.strings().err_receive_dir_picker
                    ))
                }
            }
        }
        cx.notify();
    }
    fn finish_name_edit(&mut self, window: &mut Window) {
        self.name_input_subscription = None;
        self.name_input = None;
        self.focus.focus(window);
    }
    fn cancel_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_name_edit(window);
        self.settings_error = None;
        self.name_saved = false;
        cx.notify();
    }
    fn save_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.name_input.as_ref() else {
            return;
        };
        if input.read(cx).is_composing() || self.receive.active.is_some() {
            return;
        }
        let draft = input.read(cx).text().to_owned();
        let result =
            crate::settings::path().and_then(|path| crate::settings::save_to(&path, &draft));
        match result {
            Ok(name) => {
                if self.device_name != name {
                    self.receiver_status = Err(self.language.strings().updating_broadcast.into());
                }
                self.device_name = name;
                self.receiver.configure(
                    self.model
                        .connection_ready()
                        .then(|| self.device_name.clone()),
                );
                self.finish_name_edit(window);
                self.settings_error = None;
                self.name_saved = true;
            }
            Err(error) => {
                self.settings_error =
                    Some(error.downcast_ref::<wayline_host::NameError>().map_or_else(
                        || format!("{}{error:#}", self.language.strings().err_saving_name),
                        |kind| self.language.name_error(*kind),
                    ))
            }
        }
        cx.notify();
    }
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.firmware.busy
            || self.model.phase.busy()
            || self.picker_open
            || self.name_input.is_some()
            || self.incoming.is_some()
        {
            return;
        }
        if !self.settings_open && !self.radio_change_pending {
            self.radio_draft = None;
        }
        self.settings_open = true;
        self.pending_send_layout = None;
        self.history_open = false;
        self.language_menu_open = false;
        self.notification_position_menu_open = false;
        self.notification_duration_menu_open = false;
        self.firmware.open = false;
        self.name_saved = false;
        self.focus.focus(window);
        cx.notify();
    }
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_send_layout.is_some() {
            self.dismiss_send_layout_refresh(cx);
        } else if self.firmware.open {
            self.firmware.open = false;
            self.firmware.confirm_exit = false;
            cx.notify();
        } else if self.history_open && self.history_deletion.is_some() {
            self.cancel_history_deletion(cx);
        } else if self.history_open && self.history_detail.is_some() {
            self.history_detail = None;
            cx.notify();
        } else if self.history_open {
            self.history_open = false;
            self.focus.focus(window);
            cx.notify();
        } else if self.settings_open {
            self.settings_open = false;
            self.language_menu_open = false;
            self.notification_position_menu_open = false;
            self.notification_duration_menu_open = false;
            self.finish_name_edit(window);
            self.name_saved = false;
            self.focus.focus(window);
            cx.notify();
        } else if self.share_result.is_some() {
            self.finish_share_result(window, cx);
        } else {
            self.reset(window, cx);
        }
    }
    fn accept_files(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.drag_active = false;
        self.expand(true, window, cx);
        if self.firmware.busy || self.model.phase.busy() {
            self.model.error = Some(self.language.strings().busy_device_add_files.into());
            cx.notify();
            return;
        }
        // Every new picker/drop/Explorer batch replaces the previous selection.
        // Retaining an older batch here could send files the user did not select.
        let mut selection = Vec::new();
        for path in paths {
            if !selection.contains(&path) {
                selection.push(path);
            }
        }
        if selection.is_empty() {
            return;
        }
        self.receive.dismiss_notice();
        self.model.selection_id += 1;
        self.model.validating = true;
        self.model.error = None;
        backend::validate(self.model.selection_id, selection, self.tx.clone());
        cx.notify();
    }
    fn scan(&mut self, cx: &mut Context<Self>) {
        if self.radio_change_pending
            || self.picker_open
            || self.firmware.busy
            || self.model.scanning
            || self.model.phase.busy()
        {
            return;
        }
        self.model.scan_id += 1;
        self.model.scanning = true;
        self.model.peers.clear();
        self.model.selected = None;
        self.model.error = None;
        if let Some(cancel) = self.scan_cancel.take() {
            cancel.cancel();
        }
        let cancel = SendCancellation::default();
        self.scan_cancel = Some(cancel.clone());
        self.workers.push(backend::scan(
            self.model.scan_id,
            self.device_name.clone(),
            self.use_apple_identity,
            cancel,
            self.tx.clone(),
        ));
        cx.notify();
    }
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.radio_change_pending
            || self.firmware.busy
            || self.picker_open
            || self.name_input.is_some()
            || self.queued_at.is_some()
        {
            return;
        }
        let Some(peer) = self.model.begin_send() else {
            return;
        };
        let cancel = SendCancellation::default();
        self.cancel = Some(cancel.clone());
        if let Some(cancel) = self.scan_cancel.take() {
            cancel.cancel();
        }
        self.workers.push(backend::send(
            self.model.send_id,
            peer,
            self.model.files.clone(),
            self.device_name.clone(),
            self.use_apple_identity,
            cancel,
            self.tx.clone(),
        ));
        cx.notify();
    }

    fn open_history(&mut self, cx: &mut Context<Self>) {
        if self.can_open_history() {
            self.history_open = true;
            self.history_detail = None;
            self.language_menu_open = false;
            if !self.history_deleting {
                self.history_deletion = None;
            }
            cx.notify();
        }
    }

    fn can_open_history(&self) -> bool {
        !self.firmware.open
            && !self.firmware.busy
            && !self.model.phase.busy()
            && self.receive.active.is_none()
            && self.incoming.is_none()
            && !self.picker_open
            && self.name_input.is_none()
    }

    fn request_history_deletion(
        &mut self,
        records: Vec<wayline_host::TransferHistoryRecord>,
        cx: &mut Context<Self>,
    ) {
        if records.is_empty() || self.history_deleting || self.history_preference_pending {
            return;
        }
        self.history_deletion = Some(HistoryDeletion {
            records,
            delete_files: self.history_delete_files,
            remember: false,
        });
        self.history_error = None;
        cx.notify();
    }

    fn confirm_history_deletion(&mut self, cx: &mut Context<Self>) {
        if self.history_deleting {
            return;
        }
        if let (Some(deletion), Some(worker)) = (&self.history_deletion, &self.history_worker) {
            worker.delete(
                deletion.records.clone(),
                deletion.delete_files,
                deletion.remember,
            );
            self.history_deleting = true;
            self.history_preference_pending |= deletion.remember;
            self.history_error = None;
        }
        cx.notify();
    }

    fn cancel_history_deletion(&mut self, cx: &mut Context<Self>) {
        if !self.history_deleting {
            self.history_deletion = None;
        }
        cx.notify();
    }

    fn toggle_history_delete_default(&mut self, cx: &mut Context<Self>) {
        if self.history_preference_pending || self.history_deleting {
            return;
        }
        if let Some(worker) = &self.history_worker {
            worker.save_delete_preference(!self.history_delete_files);
            self.history_preference_pending = true;
        }
        cx.notify();
    }

    fn open_history_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(shell) = &self.shell else {
            return;
        };
        if let Err(error) = shell.open_history_folder(path) {
            self.history_error = Some(format!(
                "{}{error:#}",
                self.language.strings().history_open_error
            ));
        }
        cx.notify();
    }

    fn open_history_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(shell) = &self.shell else {
            return;
        };
        if let Err(error) = shell.open_history_file(path) {
            self.history_error = Some(format!(
                "{}{error:#}",
                self.language.strings().err_open_file
            ));
        }
        cx.notify();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        self.model.phase = Phase::Cancelling;
        cx.notify();
    }
    fn clear_previews(&mut self, window: &mut Window) {
        for image in self.model.previews.drain(..).flatten() {
            let _ = window.drop_image(image);
        }
    }
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.firmware.busy || self.model.phase.busy() || self.picker_open {
            return;
        }
        self.clear_previews(window);
        self.model.reset_selection();
        self.queued_files.clear();
        self.queued_at = None;
        self.cancel = None;
        if let Some(cancel) = self.scan_cancel.take() {
            cancel.cancel();
        }
        cx.notify();
    }
    fn status_text(&self) -> (&'static str, u32) {
        let text = self.language.strings();
        match &self.model.status {
            Some(Ok(status)) if status.ready() => (text.status_connected, 0x30d158),
            Some(Ok(_)) => (text.status_waiting_bridge, 0xff9f0a),
            Some(Err(_)) => (text.status_service_unavailable, 0xff9f0a),
            None => (text.status_connecting, MUTED),
        }
    }

    fn pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let subtitle = if self.model.phase.busy() {
            self.model.phase.title(self.language)
        } else if self.model.phase == Phase::Complete {
            text.pill_delivered
        } else {
            text.pill_drop_to_send
        };
        div()
            .id("capsule")
            .w_full()
            .h(px(56.))
            .rounded_full()
            .bg(rgb(0x1c1c1f))
            .border_1()
            .border_color(rgba(0xffffff24))
            .flex()
            .items_center()
            .px(px(16.))
            .gap(px(12.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.expand(true, window, cx)))
            .when(self.drop_hovered, |pill| {
                pill.bg(rgb(0x163353)).border_color(rgb(BLUE))
            })
            .child(crate::brand::mark(30.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Wayline"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(subtitle),
                    ),
            )
            .into_any_element()
    }

    fn exit_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let error = match &self.exit_state {
            Some(ExitState::Failed(error)) => Some(error),
            _ => None,
        };
        div()
            .size_full()
            .rounded(px(crate::window_frame::CORNER_RADIUS))
            .bg(rgb(0x1c1c1f))
            .border_1()
            .border_color(rgba(0xffffff24))
            .flex()
            .flex_col()
            .p(px(22.))
            .gap(px(18.))
            .child(crate::brand::mark(30.))
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if error.is_some() {
                        text.quit_failed_title
                    } else {
                        text.quitting_title
                    }),
            )
            .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(
                error.cloned().unwrap_or_else(|| {
                    if self
                        .model
                        .status
                        .as_ref()
                        .and_then(|status| status.as_ref().ok())
                        .is_some_and(|status| {
                            status.rtl8821cu_width_mhz.is_some() && status.state == "starting"
                        })
                    {
                        text.quit_waiting_usb.into()
                    } else {
                        text.quit_detail.into()
                    }
                }),
            ))
            .when(error.is_some(), |panel| {
                panel.child(
                    button("retry-quit", text.retry_quit, true)
                        .on_click(cx.listener(|this, _, window, cx| this.quit(window, cx))),
                )
            })
            .when(error.is_none(), |panel| {
                panel.child(
                    button("quit-in-background", text.quit_in_background, false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.visibility = Visibility::Hidden;
                            this.place();
                            cx.notify();
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    fn panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let (status, color) = self.status_text();
        let usb_startup = self.model.status.as_ref().and_then(|link| {
            let link = link.as_ref().ok()?;
            link.rtl8821cu_startup
                .as_ref()
                .filter(|_| link.state == "starting")
        });
        let compact_theme = self.send_layout == SendLayout::NekoLight
            && !self.settings_open
            && !self.history_open
            && !self.firmware.open
            && self.share_result.is_none()
            && self.incoming.is_none();
        let neko_dark = self.send_layout == SendLayout::NekoDark;
        let backdrop = if compact_theme {
            0xf8fafc
        } else if neko_dark {
            0x111821
        } else {
            0x1c1c1f
        };
        let surface = if compact_theme { 0xe9eef4 } else { SURFACE };
        let hover = if compact_theme { 0xdce8f6 } else { 0x393940 };
        let ink = if compact_theme { 0x172538 } else { INK };
        let muted = if compact_theme { 0x4d5d74 } else { MUTED };
        let show_back = self.incoming.is_none()
            && (self.settings_open
                || self.history_open
                || self.firmware.open
                || (self.model.phase == Phase::Ready
                    && (!self.model.files.is_empty() || self.model.validating)));
        div()
            .id("panel")
            .size_full()
            .rounded(px(crate::window_frame::CORNER_RADIUS))
            .bg(rgb(backdrop))
            .border_1()
            .border_color(rgba(if compact_theme {
                0x15253b35
            } else {
                0xffffff24
            }))
            .flex()
            .flex_col()
            .p(px(22.))
            .overflow_hidden()
            .relative()
            .when(self.drop_hovered, |panel| {
                panel.border_color(rgb(BLUE)).bg(rgb(if compact_theme {
                    0xe6f0fc
                } else {
                    0x202b38
                }))
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .mb(px(if show_back { 4. } else { 18. }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .child(crate::brand::mark(25.))
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(ink))
                                    .child("Wayline"),
                            ),
                    )
                    .child(div().id("window-drag-spacer").flex_1().min_w_0().h(px(28.)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(
                                !self.settings_open
                                    && !self.history_open
                                    && !self.firmware.open
                                    && self.incoming.is_none(),
                                |controls| {
                                    controls.child(
                                        div()
                                            .id("open-firmware")
                                            .size(px(28.))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(rgb(surface))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(muted))
                                            .child(firmware_icon())
                                            .tooltip(move |window, cx| {
                                                icon_tooltip(text.firmware_title, window, cx)
                                            })
                                            .when(self.can_open_firmware(), |control| {
                                                control.cursor_pointer().hover(move |s| {
                                                    s.bg(rgb(hover)).text_color(rgb(ink))
                                                })
                                            })
                                            .when(!self.can_open_firmware(), |control| {
                                                control.opacity(0.4)
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_firmware(window, cx)
                                            })),
                                    )
                                },
                            )
                            .when(
                                !self.settings_open
                                    && !self.history_open
                                    && !self.firmware.open
                                    && self.incoming.is_none(),
                                |controls| {
                                    controls.child(
                                        div()
                                            .id("history")
                                            .size(px(28.))
                                            .rounded_full()
                                            .bg(rgb(surface))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(rgb(muted))
                                            .child(history_icon())
                                            .tooltip(move |window, cx| {
                                                icon_tooltip(text.history_title, window, cx)
                                            })
                                            .when(self.can_open_history(), |control| {
                                                control.cursor_pointer().hover(move |s| {
                                                    s.bg(rgb(hover)).text_color(rgb(ink))
                                                })
                                            })
                                            .when(!self.can_open_history(), |control| {
                                                control.opacity(0.4)
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.open_history(cx)),
                                            ),
                                    )
                                },
                            )
                            .when(!self.settings_open && !self.history_open, |controls| {
                                controls.child(
                                    div()
                                        .id("settings")
                                        .size(px(28.))
                                        .rounded_full()
                                        .bg(rgb(surface))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(rgb(muted))
                                        .child(settings_gear())
                                        .tooltip(move |window, cx| {
                                            icon_tooltip(text.settings_title, window, cx)
                                        })
                                        .when(
                                            !self.model.phase.busy()
                                                && !self.picker_open
                                                && self.name_input.is_none(),
                                            |control| {
                                                control.cursor_pointer().hover(move |s| {
                                                    s.bg(rgb(hover)).text_color(rgb(ink))
                                                })
                                            },
                                        )
                                        .when(
                                            self.model.phase.busy()
                                                || self.picker_open
                                                || self.name_input.is_some(),
                                            |control| control.opacity(0.4),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_settings(window, cx)
                                        })),
                                )
                            })
                            .child({
                                let pinned = self.pinned;
                                div()
                                    .id("pin")
                                    .cursor_pointer()
                                    .size(px(28.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(pinned, |control| {
                                        control
                                            .bg(rgb(if compact_theme {
                                                0xd9e9fa
                                            } else {
                                                0x182c40
                                            }))
                                            .text_color(rgb(BLUE))
                                            .hover(move |s| {
                                                s.bg(rgb(if compact_theme {
                                                    0xcbe1fa
                                                } else {
                                                    0x223d59
                                                }))
                                            })
                                    })
                                    .when(!pinned, |control| {
                                        control
                                            .bg(rgb(surface))
                                            .text_color(rgb(muted))
                                            .hover(move |s| s.bg(rgb(hover)).text_color(rgb(ink)))
                                    })
                                    .child(pin_icon(pinned))
                                    .tooltip(move |window, cx| {
                                        icon_tooltip(
                                            if pinned {
                                                text.unpin_window
                                            } else {
                                                text.pin_window
                                            },
                                            window,
                                            cx,
                                        )
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_pinned(cx);
                                    }))
                            })
                            .child(
                                div()
                                    .id("close")
                                    .cursor_pointer()
                                    .size(px(28.))
                                    .rounded_full()
                                    .bg(rgb(surface))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(muted))
                                    .text_size(px(20.))
                                    .child("×")
                                    .hover(move |s| s.bg(rgb(hover)))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.expand(false, window, cx)
                                    })),
                            ),
                    ),
            )
            .children(self.settings_error.as_ref().map(|error| {
                if self.send_layout != SendLayout::Classic {
                    self.compact_send_error(error)
                } else {
                    self.error_view(error)
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(show_back, |content| {
                        content.child(
                            div()
                                .relative()
                                .flex()
                                .flex_shrink_0()
                                .w_full()
                                .h(px(24.))
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .id("back")
                                        .h(px(24.))
                                        .px(px(8.))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .rounded(px(8.))
                                        .cursor_pointer()
                                        .text_color(rgb(muted))
                                        .child(back_chevron())
                                        .child(div().text_size(px(11.)).line_height(px(14.)).child(
                                            if self.history_open {
                                                text.back
                                            } else if self.settings_open || self.firmware.open {
                                                text.back_to_share
                                            } else {
                                                text.back
                                            },
                                        ))
                                        .hover(move |s| s.bg(rgb(surface)).text_color(rgb(ink)))
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.back(window, cx)
                                            }),
                                        ),
                                )
                                .when(
                                    self.settings_open || self.history_open || self.firmware.open,
                                    |row| {
                                        row.child(
                                            div()
                                                .id("page-title")
                                                .absolute()
                                                .left(px(80.))
                                                .right(px(80.))
                                                .top(px(0.))
                                                .h(px(24.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(13.))
                                                .line_height(px(18.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(if self.history_open {
                                                    text.history_title
                                                } else if self.firmware.open {
                                                    text.firmware_title
                                                } else {
                                                    text.settings_title
                                                }),
                                        )
                                    },
                                ),
                        )
                    })
                    .child(if self.incoming.is_some() {
                        self.incoming_view(cx)
                    } else if self.settings_open && self.pending_send_layout.is_some() {
                        self.send_layout_refresh_view(cx)
                    } else if self.firmware.open {
                        self.firmware_view(cx)
                    } else if self.history_open {
                        self.history_view(cx)
                    } else if self.settings_open {
                        self.settings_view(cx)
                    } else if self.share_result.is_some() && !self.model.phase.busy() {
                        self.share_result_view(cx)
                    } else if self.model.phase == Phase::Ready
                        && self.receive.active.is_some()
                        && self.model.files.is_empty()
                    {
                        self.receiving_view(false)
                    } else {
                        self.send_page(cx)
                    }),
            )
            .when(self.incoming.is_none(), |panel| {
                panel
                    .when(
                        self.receive.active.is_some()
                            && (self.model.phase != Phase::Ready
                                || !self.model.files.is_empty()
                                || self.settings_open
                                || self.history_open
                                || self.firmware.open),
                        |panel| panel.child(self.receiving_view(true)),
                    )
                    .when(self.receiver_declined_until.is_some(), |panel| {
                        panel.child(
                            div()
                                .id("receiver-declined-notice")
                                .w_full()
                                .text_center()
                                .text_size(px(10.))
                                .text_color(rgb(muted))
                                .child(text.receiver_declined),
                        )
                    })
            })
            .when_some(
                usb_startup.filter(|_| {
                    !self.settings_open && !self.firmware.open && self.incoming.is_none()
                }),
                |panel, startup| panel.child(self.usb_startup_progress(startup, muted, surface)),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .mt(px(18.))
                    .justify_between()
                    .items_center()
                    .text_size(px(10.))
                    .text_color(rgb(muted))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(5.)).rounded_full().bg(rgb(color)))
                            .child(status),
                    )
                    .child(if self.name_input.is_some() {
                        text.footer_save
                    } else if self.settings_open || self.history_open || self.firmware.open {
                        text.footer_back
                    } else if self.picker_open {
                        text.footer_choosing
                    } else if self.model.validating {
                        text.footer_reading
                    } else {
                        text.footer_choose
                    }),
            )
            .children(
                (self.incoming.is_none()
                    && self.share_result.is_none()
                    && !self.settings_open
                    && !self.history_open
                    && !self.firmware.open)
                    .then(|| self.receive_notice_view(cx))
                    .flatten(),
            )
            .into_any_element()
    }

    fn can_open_firmware(&self) -> bool {
        !self.model.phase.busy()
            && !self.picker_open
            && self.name_input.is_none()
            && self.incoming.is_none()
    }

    fn open_firmware(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_firmware() {
            return;
        }
        self.finish_name_edit(window);
        self.settings_open = false;
        self.pending_send_layout = None;
        self.history_open = false;
        self.language_menu_open = false;
        self.firmware.open = true;
        if !self.firmware.busy && !self.firmware.loading {
            self.firmware.loading = true;
            crate::firmware::refresh(self.tx.clone());
        }
        cx.notify();
    }
    fn firmware_transfer_active(&self) -> bool {
        self.radio_change_pending
            || self.model.phase.busy()
            || self.receive.active.is_some()
            || self.incoming.is_some()
            || self.receiver.has_active_transfers()
    }
    fn start_firmware(&mut self, cx: &mut Context<Self>) {
        if !self.firmware.can_start(self.firmware_transfer_active()) {
            return;
        }
        let board = self.firmware.package.as_ref().unwrap().board.clone();
        let paths = wayline_flash::installed_manifest_for_board(&board)
            .and_then(|manifest| Ok((manifest, wayline_flash::installed_tool()?)));
        let (manifest_path, tool_path) = match paths {
            Ok(paths) => paths,
            Err(error) => {
                self.firmware.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        if let Some(cancel) = self.scan_cancel.take() {
            cancel.cancel();
        }
        self.model.scan_id += 1;
        self.model.scanning = false;
        self.firmware.busy = true;
        self.firmware.ready = false;
        self.firmware.confirm_exit = false;
        self.firmware.stage = None;
        self.firmware.progress = None;
        self.firmware.cancelling = false;
        self.firmware.error = None;
        self.firmware.message = self.language.strings().firmware_prepare_maintenance.into();
        let cancel = wayline_flash::Cancellation::default();
        self.firmware_cancel = Some(cancel.clone());
        let options = wayline_flash::FlashOptions {
            manifest_path,
            tool_path,
            port: self.firmware.selected.clone().unwrap(),
            board: self.firmware.package.as_ref().unwrap().board.clone(),
            mode: self.firmware.mode,
        };
        self.firmware_worker = Some(crate::firmware::start(
            options,
            cancel,
            self.receiver.shutdown(),
            std::mem::take(&mut self.workers),
            self.tx.clone(),
        ));
        cx.notify();
    }
    fn firmware_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let transfer_active = self.firmware_transfer_active();
        let can_start = self.firmware.can_start(transfer_active);
        let mut body = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                firmware_card()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.firmware.package.as_ref().map_or_else(
                                || text.firmware_install.to_string(),
                                |package| {
                                    wayline_flash::profile_for_board(&package.board).map_or_else(
                                        || package.board.clone(),
                                        |profile| profile.label.into(),
                                    )
                                },
                            )),
                    )
                    .child(firmware_hint(
                        self.firmware.package.as_ref().map_or_else(
                            || text.firmware_not_found.to_string(),
                            |package| {
                                format!(
                                    "{} · {} MiB",
                                    package.chip,
                                    package.flash_size / (1024 * 1024)
                                )
                            },
                        ),
                        MUTED,
                    ))
                    .child(firmware_hint(
                        self.language
                            .firmware_package_version(self.firmware.version.as_deref()),
                        MUTED,
                    )),
            );
        if self.firmware.packages.len() > 1 {
            let mut profiles = div().flex().flex_wrap().gap(px(6.));
            for (index, package) in self.firmware.packages.iter().enumerate() {
                let board = package.board.clone();
                let selected = self
                    .firmware
                    .package
                    .as_ref()
                    .is_some_and(|current| current.board == board);
                let label = wayline_flash::profile_for_board(&board)
                    .map_or_else(|| board.clone(), |profile| profile.label.into());
                profiles = profiles.child(
                    div()
                        .id(("firmware-profile", index))
                        .h(px(32.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgb(if selected { BLUE } else { 0x3a3a40 }))
                        .bg(rgb(if selected { 0x163353 } else { 0x202024 }))
                        .flex()
                        .items_center()
                        .text_size(px(10.))
                        .text_color(rgb(if selected { INK } else { MUTED }))
                        .child(label)
                        .when(self.firmware.busy, |button| button.opacity(0.4))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.firmware.select_profile(&board) {
                                cx.notify();
                            }
                        })),
                );
            }
            body = body.child(
                firmware_card().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(text.firmware_target),
                        )
                        .child(profiles),
                ),
            );
        }
        if self.firmware.confirm_exit && self.firmware.busy {
            body = body.child(
                firmware_card()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(text.firmware_stop_quit),
                    )
                    .child(firmware_hint(text.firmware_stop_quit_detail, 0xffb56b)),
            );
            let actions = div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().child(
                    firmware_button("keep-flashing", text.firmware_keep_install, true).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.firmware.confirm_exit = false;
                            cx.notify();
                        }),
                    ),
                ))
                .child(
                    div().flex_1().min_w_0().child(
                        firmware_button("cancel-flash-quit", text.firmware_cancel_quit, false)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.begin_quit(true, window, cx)
                                }),
                            ),
                    ),
                );
            return crate::share_layout::page_with_actions(
                "firmware-page",
                body.into_any_element(),
                actions.into_any_element(),
            )
            .into_any_element();
        }
        if self.firmware.busy {
            let mut status = firmware_card()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(self.firmware.stage_label()),
                        )
                        .children(self.firmware.progress.map(|progress| {
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(format!("{}%", progress.min(100)))
                        })),
                )
                .child(firmware_hint(self.firmware.message.clone(), MUTED));
            if let Some(progress) = self.firmware.progress {
                status = status.child(
                    div()
                        .h(px(5.))
                        .flex_shrink_0()
                        .w_full()
                        .rounded_full()
                        .bg(rgb(0x3a3a40))
                        .child(
                            div()
                                .h_full()
                                .w(relative(progress.min(100) as f32 / 100.))
                                .rounded_full()
                                .bg(rgb(BLUE)),
                        ),
                );
            }
            body = body
                .child(status)
                .child(firmware_hint(text.firmware_write_warning, 0xffb56b));
            let action = firmware_button(
                "cancel-firmware",
                if self.firmware.cancelling {
                    text.firmware_stopping
                } else {
                    text.firmware_cancel
                },
                false,
            )
            .when(self.firmware.cancelling, |button| {
                button.opacity(0.4).cursor(CursorStyle::Arrow)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.firmware.busy && !this.firmware.cancelling {
                    if let Some(cancel) = &this.firmware_cancel {
                        cancel.cancel();
                    }
                    this.firmware.cancelling = true;
                    cx.notify();
                }
            }));
            return crate::share_layout::page_with_actions(
                "firmware-page",
                body.into_any_element(),
                action.into_any_element(),
            )
            .into_any_element();
        }
        if self.firmware.ready || self.firmware.error.is_some() {
            let status = match &self.firmware.error {
                Some(error) => firmware_hint(error.clone(), 0xffb56b),
                None => firmware_hint(self.firmware.message.clone(), 0x30d158),
            };
            body = body.child(firmware_card().child(status));
        }
        let mut devices = firmware_card().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(text.firmware_connect),
                )
                .child(
                    div()
                        .id("refresh-firmware")
                        .h(px(24.))
                        .px(px(6.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .text_color(rgb(BLUE))
                        .child(if self.firmware.loading {
                            text.firmware_checking
                        } else {
                            text.firmware_refresh
                        })
                        .when(!self.firmware.loading, |button| {
                            button.cursor_pointer().hover(|s| s.bg(rgb(0x393940)))
                        })
                        .when(self.firmware.loading, |button| button.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.firmware.loading && !this.firmware.busy {
                                this.firmware.loading = true;
                                crate::firmware::refresh(this.tx.clone());
                                cx.notify();
                            }
                        })),
                ),
        );
        if self.firmware.devices.is_empty() {
            devices = devices.child(firmware_hint(
                if self.firmware.loading {
                    text.firmware_searching
                } else {
                    text.firmware_no_port
                },
                MUTED,
            ));
        }
        for (index, device) in self.firmware.devices.iter().enumerate() {
            let port = device.port.clone();
            let selected = self.firmware.selected.as_ref() == Some(&port);
            devices = devices.child(
                div()
                    .id(("firmware-port", index))
                    .w_full()
                    .min_w_0()
                    .p(px(8.))
                    .flex_shrink_0()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if selected { BLUE } else { 0x3a3a40 }))
                    .bg(rgb(if selected { 0x163353 } else { 0x202024 }))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(if selected { BLUE } else { MUTED }))
                            .child(if selected { "●" } else { "○" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(port.clone()),
                            )
                            .child(firmware_hint(device.label.clone(), MUTED)),
                    )
                    .when(!self.firmware.loading, |row| row.cursor_pointer())
                    .when(self.firmware.loading, |row| row.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.firmware.loading || this.firmware.busy {
                            return;
                        }
                        if this.firmware.selected.as_ref() != Some(&port) {
                            this.firmware.board_confirmed = false;
                            this.firmware.ready = false;
                        }
                        this.firmware.selected = Some(port.clone());
                        cx.notify();
                    })),
            );
        }
        body = body.child(devices);
        if self.firmware.selected.is_some()
            && self.firmware.package.is_some()
            && !self.firmware.profile_matches_selected_port(
                self.firmware.package.as_ref().map(|p| p.board.as_str()),
            )
        {
            body = body.child(firmware_hint(text.firmware_profile_mismatch, 0xffb56b));
        }
        let upgrade = self.firmware.mode == wayline_flash::InstallMode::Upgrade;
        let mut mode_buttons = div().flex().gap(px(6.));
        for (id, label, mode) in [
            (
                "firmware-upgrade",
                text.firmware_keep_settings,
                wayline_flash::InstallMode::Upgrade,
            ),
            (
                "firmware-reinstall",
                text.firmware_reinstall,
                wayline_flash::InstallMode::Reinstall,
            ),
        ] {
            let selected = self.firmware.mode == mode;
            mode_buttons = mode_buttons.child(
                div()
                    .id(id)
                    .flex_1()
                    .min_w_0()
                    .h(px(32.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if selected { BLUE } else { 0x3a3a40 }))
                    .bg(rgb(if selected { 0x163353 } else { 0x202024 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgb(if selected { INK } else { MUTED }))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.firmware.busy {
                            this.firmware.mode = mode;
                            cx.notify();
                        }
                    })),
            );
        }
        body = body.child(
            firmware_card()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(text.firmware_options),
                )
                .child(mode_buttons)
                .child(firmware_hint(
                    if upgrade {
                        text.firmware_keep_hint
                    } else {
                        text.firmware_replace_hint
                    },
                    if upgrade { MUTED } else { 0xffb56b },
                ))
                .child(
                    div()
                        .id("confirm-board")
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(16.))
                                .flex_shrink_0()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(rgb(if self.firmware.board_confirmed {
                                    BLUE
                                } else {
                                    MUTED
                                }))
                                .bg(rgb(if self.firmware.board_confirmed {
                                    BLUE
                                } else {
                                    0x202024
                                }))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.))
                                .text_color(rgb(0xffffff))
                                .child(if self.firmware.board_confirmed {
                                    "✓"
                                } else {
                                    ""
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.))
                                .whitespace_normal()
                                .child(text.firmware_confirm_board),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.firmware.busy {
                                this.firmware.board_confirmed = !this.firmware.board_confirmed;
                                cx.notify();
                            }
                        })),
                )
                .child(firmware_hint(text.firmware_chip_hint, MUTED)),
        );
        if transfer_active {
            body = body.child(firmware_hint(text.firmware_transfer_active, 0xffb56b));
        }
        let action = firmware_button(
            "install-firmware",
            if self.firmware.error.is_some() {
                text.firmware_retry
            } else if upgrade {
                text.firmware_start_update
            } else {
                text.firmware_start_install
            },
            true,
        )
        .when(!can_start, |button| {
            button.opacity(0.4).cursor(CursorStyle::Arrow)
        })
        .on_click(cx.listener(|this, _, _, cx| this.start_firmware(cx)));
        crate::share_layout::page_with_actions(
            "firmware-page",
            body.into_any_element(),
            action.into_any_element(),
        )
        .into_any_element()
    }

    fn radio_settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let status = self.radio_status();
        let selection = self.radio_selection();
        let can_save = self
            .radio_draft
            .is_some_and(|draft| self.can_save_radio(draft));
        let channel = status.map(|status| status.channel);
        let target = status.and_then(|status| status.target.as_deref());
        let target_label = match target {
            Some("esp32s3") => "ESP32-S3",
            Some("esp32c5") => "ESP32-C5",
            Some("rtl8821cu") => "RTL8821CU (USB)",
            _ => text.radio_unknown_board,
        };
        let current_label = channel.map(|channel| match channel {
            6 => text.channel_6,
            44 => text.channel_44,
            149 => text.channel_149,
            _ => text.channel_unknown,
        });
        let mut status_text = match (status, current_label) {
            (Some(status), Some(current)) if status.ready() => {
                format!("{target_label} · {} {current}", text.radio_current)
            }
            (Some(_), Some(current)) => format!(
                "{target_label} · {} {current} · {}",
                text.radio_configured, text.radio_not_ready
            ),
            (Some(_), None) => {
                format!("{target_label} · {}", text.radio_current_unknown)
            }
            _ => text.radio_reading.into(),
        };
        if let Some(width) = status.and_then(|status| status.rtl8821cu_width_mhz) {
            status_text.push_str(&format!(" · {width} MHz"));
        }
        let has_5ghz = matches!(target, Some("esp32c5" | "rtl8821cu"))
            || status.is_some_and(|status| status.rtl8821cu_width_mhz.is_some());
        let choices: &[u16] = if has_5ghz { &[6, 44, 149] } else { &[6] };
        div()
            .flex_shrink_0()
            .rounded(px(16.))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(13.)).child(text.radio_title))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(status_text),
            )
            .when_some(
                status.and_then(|status| status.rtl8821cu_startup.as_ref()),
                |card, startup| card.child(self.usb_startup_progress(startup, MUTED, 0x393940)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children(choices.iter().copied().map(|channel| {
                        let selected = selection.is_some_and(|draft| channel == draft.channel);
                        let enabled = self.can_edit_radio() && selection.is_some();
                        let label = match channel {
                            6 => text.channel_6,
                            44 => text.channel_44,
                            _ => text.channel_149,
                        };
                        div()
                            .id(("radio-channel", channel as u32))
                            .h(px(34.))
                            .px(px(12.))
                            .rounded(px(8.))
                            .bg(rgb(if selected { BLUE } else { 0x393940 }))
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label)
                            .when(!enabled, |button| {
                                button
                                    .opacity(if selected { 0.65 } else { 0.4 })
                                    .cursor(CursorStyle::Arrow)
                            })
                            .when(enabled, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x484850)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_radio_channel(channel, cx);
                                    }))
                            })
                    })),
            )
            .when_some(
                selection.filter(|draft| draft.width_mhz.is_some()),
                |view, draft| view.child(self.radio_width_controls(draft, cx)),
            )
            .when(
                selection.is_some_and(|draft| draft.channel == 44 && draft.width_mhz.is_some()),
                |view| {
                    view.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(text.radio_channel_44_hint),
                    )
                },
            )
            .child(
                button("save-radio", text.save, can_save)
                    .when(!can_save, |button| {
                        button.opacity(0.5).cursor(CursorStyle::Arrow)
                    })
                    .when(can_save, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| this.save_radio(cx)))
                    }),
            )
            .when(self.radio_change_pending, |view| {
                view.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(text.radio_restarting),
                )
            })
            .into_any_element()
    }

    fn usb_startup_progress(
        &self,
        startup: &wayline_service::protocol::Rtl8821cuStartup,
        muted: u32,
        track: u32,
    ) -> impl IntoElement {
        use wayline_service::protocol::Rtl8821cuStartupPhase as Phase;
        let text = self.language.strings();
        let label = match startup.phase {
            Phase::BridgeLaunch => text.usb_start_bridge,
            Phase::UsbHelperInitialization => text.usb_start_helper,
            Phase::HelloVerification => text.usb_start_verify,
            Phase::WintunSetup => text.usb_start_wintun,
            Phase::Ready => text.usb_start_ready,
        };
        let percent = startup.percent.min(100);
        let duration = self
            .language
            .transfer_elapsed(startup.elapsed_seconds.saturating_mul(1000));
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(5.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .child(div().min_w_0().flex_1().child(label))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(BLUE))
                            .child(format!("{percent}%")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(track))
                    .child(
                        div()
                            .h_full()
                            .w(relative(f32::from(percent) / 100.))
                            .rounded_full()
                            .bg(rgb(BLUE)),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(muted))
                    .child(duration),
            )
    }

    fn radio_width_controls(&self, draft: RadioChange, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.language.strings();
        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(div().text_size(px(12.)).child(text.radio_width_title))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children([20, 40, 80].map(|width| {
                        let selected = draft.width_mhz == Some(width);
                        let enabled = self.can_edit_radio()
                            && (width == 20 || matches!(draft.channel, 44 | 149));
                        div()
                            .id(("rtl8821cu-width", u32::from(width)))
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(px(8.))
                            .bg(rgb(if selected { BLUE } else { 0x393940 }))
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .child(format!("{width} MHz"))
                            .when(!enabled && !selected, |button| button.opacity(0.4))
                            .when(enabled, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x484850)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_radio_width(width, cx);
                                    }))
                            })
                    })),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .whitespace_normal()
                    .child(text.radio_width_hint),
            )
    }

    fn language_settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        div()
            .flex_shrink_0()
            .rounded(px(16.))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(13.)).child(text.language_title))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.language_menu_open {
                            this.language_menu_open = false;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .id("language-select")
                            .h(px(36.))
                            .w_full()
                            .px(px(12.))
                            .rounded(px(9.))
                            .border_1()
                            .border_color(rgb(if self.language_menu_open {
                                BLUE
                            } else {
                                0x3a3a40
                            }))
                            .bg(rgb(0x202024))
                            .flex()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .hover(|style| style.bg(rgb(0x2a2a31)))
                            .child(self.language.label())
                            .child(dropdown_chevron(self.language_menu_open))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.language_menu_open = !this.language_menu_open;
                                cx.notify();
                            })),
                    )
                    .when(self.language_menu_open, |field| {
                        field.child(
                            div()
                                .id("language-menu")
                                .w_full()
                                .max_h(px(240.))
                                .visible_scrollbar()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(rgba(0xffffff18))
                                .bg(rgb(0x1c1c1f))
                                .p(px(4.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .children(Language::ALL.into_iter().map(|language| {
                                    let selected = self.language == language;
                                    div()
                                        .id(("language-option", language as u32))
                                        .h(px(30.))
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .rounded(px(6.))
                                        .bg(rgb(if selected { 0x163353 } else { 0x1c1c1f }))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .text_color(rgb(if selected { INK } else { MUTED }))
                                        .hover(|style| style.bg(rgb(0x2f2f36)))
                                        .child(language.label())
                                        .when(selected, |row| {
                                            row.child(div().text_color(rgb(BLUE)).child("✓"))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_language(language, cx)
                                        }))
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(text.language_hint),
            )
            .into_any_element()
    }

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.language_menu_open = false;
        if self.language == language {
            cx.notify();
            return;
        }
        let result = crate::settings::path()
            .and_then(|path| crate::settings::save_language_to(&path, language.code()));
        match result {
            Ok(_) => {
                self.language = language;
                i18n::set_current(language);
                self.settings_error = None;
                if !self.firmware.busy {
                    self.firmware.message = language.strings().firmware_idle.into();
                }
                if let Some(shell) = self.shell.as_mut() {
                    shell.language_changed();
                }
                // Explorer text is installed in the registry and Send to shortcut.
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    if let Err(error) = crate::integration::register() {
                        let message =
                            format!("{}{error:#}", i18n::current().strings().shell_menu_failed);
                        let _ = tx.send(Event::ShellError(message));
                    }
                });
            }
            Err(error) => {
                self.settings_error =
                    Some(format!("{}{error:#}", language.strings().err_language_save));
            }
        }
        cx.notify();
    }

    fn set_notification_position(
        &mut self,
        position: crate::settings::NotificationPosition,
        cx: &mut Context<Self>,
    ) {
        if self.notification_position == position {
            return;
        }
        let result = crate::settings::path()
            .and_then(|path| crate::settings::save_notification_position_to(&path, position));
        match result {
            Ok(position) => {
                self.notification_position = position;
                self.settings_error = None;
            }
            Err(error) => {
                self.settings_error = Some(format!(
                    "{}{error:#}",
                    self.language.strings().err_notification_position_save
                ));
            }
        }
        cx.notify();
    }

    fn set_notification_duration(&mut self, seconds: u64, cx: &mut Context<Self>) {
        if self.notification_duration_seconds == seconds {
            self.notification_duration_menu_open = false;
            cx.notify();
            return;
        }
        let result = crate::settings::path().and_then(|path| {
            crate::settings::save_notification_duration_seconds_to(&path, seconds)
        });
        match result {
            Ok(seconds) => {
                self.notification_duration_seconds = seconds;
                self.settings_error = None;
            }
            Err(error) => {
                self.settings_error = Some(format!(
                    "{}{error:#}",
                    self.language.strings().err_notification_duration_save
                ));
            }
        }
        self.notification_duration_menu_open = false;
        cx.notify();
    }

    fn set_desktop_notifications_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let result = crate::settings::path().and_then(|path| {
            crate::settings::save_desktop_notifications_enabled_to(&path, enabled)
        });
        match result {
            Ok(enabled) => {
                self.desktop_notifications_enabled = enabled;
                self.settings_error = None;
            }
            Err(error) => {
                self.settings_error = Some(format!(
                    "{}{error:#}",
                    self.language.strings().err_notification_enabled_save
                ));
            }
        }
        cx.notify();
    }

    fn notification_position_label(&self) -> &'static str {
        let text = self.language.strings();
        match self.notification_position {
            crate::settings::NotificationPosition::TopLeft => text.notice_position_top_left,
            crate::settings::NotificationPosition::TopCenter => text.notice_position_top_center,
            crate::settings::NotificationPosition::TopRight => text.notice_position_top_right,
            crate::settings::NotificationPosition::BottomLeft => text.notice_position_bottom_left,
            crate::settings::NotificationPosition::BottomCenter => {
                text.notice_position_bottom_center
            }
            crate::settings::NotificationPosition::BottomRight => text.notice_position_bottom_right,
        }
    }

    fn notification_position_select(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let options = [
            (
                crate::settings::NotificationPosition::TopLeft,
                text.notice_position_top_left,
            ),
            (
                crate::settings::NotificationPosition::TopCenter,
                text.notice_position_top_center,
            ),
            (
                crate::settings::NotificationPosition::TopRight,
                text.notice_position_top_right,
            ),
            (
                crate::settings::NotificationPosition::BottomLeft,
                text.notice_position_bottom_left,
            ),
            (
                crate::settings::NotificationPosition::BottomCenter,
                text.notice_position_bottom_center,
            ),
            (
                crate::settings::NotificationPosition::BottomRight,
                text.notice_position_bottom_right,
            ),
        ];
        let open = self.notification_position_menu_open;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.notification_position_menu_open {
                    this.notification_position_menu_open = false;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("notification-position-select")
                    .h(px(36.))
                    .w_full()
                    .px(px(12.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgb(if open { BLUE } else { 0x3a3a40 }))
                    .bg(rgb(0x202024))
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .hover(|style| style.bg(rgb(0x2a2a31)))
                    .child(self.notification_position_label())
                    .child(dropdown_chevron(open))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notification_position_menu_open =
                            !this.notification_position_menu_open;
                        this.notification_duration_menu_open = false;
                        cx.notify();
                    })),
            )
            .when(open, |field| {
                field.child(
                    div()
                        .id("notification-position-menu")
                        .w_full()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgba(0xffffff18))
                        .bg(rgb(0x1c1c1f))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(options.into_iter().enumerate().map(
                            |(index, (position, label))| {
                                let selected = self.notification_position == position;
                                div()
                                    .id(("notification-position-option", index))
                                    .h(px(30.))
                                    .flex_shrink_0()
                                    .px(px(8.))
                                    .rounded(px(6.))
                                    .bg(rgb(if selected { 0x163353 } else { 0x1c1c1f }))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(if selected { INK } else { MUTED }))
                                    .hover(|style| style.bg(rgb(0x2f2f36)))
                                    .child(label)
                                    .when(selected, |row| {
                                        row.child(div().text_color(rgb(BLUE)).child("✓"))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.notification_position_menu_open = false;
                                        this.set_notification_position(position, cx);
                                    }))
                            },
                        )),
                )
            })
            .into_any_element()
    }

    fn notification_duration_select(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.notification_duration_menu_open;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.notification_duration_menu_open {
                    this.notification_duration_menu_open = false;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("notification-duration-select")
                    .h(px(36.))
                    .w_full()
                    .px(px(12.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgb(if open { BLUE } else { 0x3a3a40 }))
                    .bg(rgb(0x202024))
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .hover(|style| style.bg(rgb(0x2a2a31)))
                    .child(
                        self.language
                            .notification_duration(self.notification_duration_seconds),
                    )
                    .child(dropdown_chevron(open))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notification_duration_menu_open =
                            !this.notification_duration_menu_open;
                        this.notification_position_menu_open = false;
                        cx.notify();
                    })),
            )
            .when(open, |field| {
                field.child(
                    div()
                        .id("notification-duration-menu")
                        .w_full()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgba(0xffffff18))
                        .bg(rgb(0x1c1c1f))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(
                            crate::settings::NOTIFICATION_DURATION_OPTIONS
                                .into_iter()
                                .map(|seconds| {
                                    let selected = self.notification_duration_seconds == seconds;
                                    div()
                                        .id(("notification-duration-option", seconds as usize))
                                        .h(px(30.))
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .rounded(px(6.))
                                        .bg(rgb(if selected { 0x163353 } else { 0x1c1c1f }))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .text_color(rgb(if selected { INK } else { MUTED }))
                                        .hover(|style| style.bg(rgb(0x2f2f36)))
                                        .child(self.language.notification_duration(seconds))
                                        .when(selected, |row| {
                                            row.child(div().text_color(rgb(BLUE)).child("✓"))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_notification_duration(seconds, cx)
                                        }))
                                }),
                        ),
                )
            })
            .into_any_element()
    }

    fn show_share_result(
        &mut self,
        title: String,
        message: String,
        original: Option<String>,
        image: Option<Arc<RenderImage>>,
        window: &mut Window,
    ) {
        let previous_visibility = self.clear_share_result(window).unwrap_or(self.visibility);
        self.share_result = Some(ShareResult {
            title,
            message,
            original,
            image,
            copied: false,
            deadline: Instant::now() + SHARE_RESULT_DURATION,
            previous_visibility,
        });
        if self.visibility != Visibility::Share {
            self.visibility = Visibility::Share;
            self.place();
        }
    }

    fn clear_share_result(&mut self, window: &mut Window) -> Option<Visibility> {
        let result = self.share_result.take()?;
        if let Some(image) = result.image {
            let _ = window.drop_image(image);
        }
        Some(result.previous_visibility)
    }

    fn finish_share_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.clear_share_result(window) == Some(Visibility::Hidden)
            && self.visibility == Visibility::Share
            && self.incoming.is_none()
            && !self.picker_open
            && !self.settings_open
            && !self.history_open
            && !self.firmware.open
            && !self.model.phase.busy()
            && self.model.files.is_empty()
        {
            self.visibility = Visibility::Hidden;
            self.place();
        }
        cx.notify();
    }

    fn show_desktop_notification(
        &mut self,
        title: &'static str,
        message: String,
        full_text: String,
        position: crate::settings::NotificationPosition,
        cx: &mut Context<Self>,
    ) {
        if !self.desktop_notifications_enabled {
            return;
        }
        if let Some(previous) = self.notification_window.take() {
            let _ = previous.update(cx, |_, window, _| window.remove_window());
        }
        let Some(owner) = self.shell.as_ref().map(Shell::notification_owner) else {
            return;
        };
        let text = self.language.strings();
        let copy = text.copy;
        let copied = text.copied;
        let duration_seconds = self.notification_duration_seconds;
        let fallback_title = title;
        let fallback_message = message.clone();
        let positioning_fallback_message = message.clone();
        let height = crate::notification::DesktopNotification::height();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(
                    px(crate::notification::DesktopNotification::width()),
                    px(height),
                ),
            ))),
            titlebar: None,
            kind: WindowKind::PopUp,
            focus: false,
            show: true,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            cx.new(|cx| {
                crate::notification::DesktopNotification::new(
                    window,
                    cx,
                    crate::notification::NotificationContent {
                        title: title.into(),
                        message: message.clone(),
                        full_text,
                        copy_label: copy,
                        copied_label: copied,
                        duration_seconds,
                    },
                )
            })
        }) {
            Ok(notification) => {
                let positioned = notification;
                cx.defer(move |cx| {
                    let position_result = positioned
                        .update(cx, |_, window, _| {
                            crate::native::queue_notification_position(
                                owner,
                                window,
                                position,
                                crate::notification::DesktopNotification::width(),
                                height,
                                fallback_title,
                                &positioning_fallback_message,
                            )
                        })
                        .and_then(|result| result);
                    if let Err(error) = position_result {
                        eprintln!("无法定位 Windows 桌面通知：{error:#}");
                        let _ = positioned.update(cx, |_, window, _| window.remove_window());
                        crate::native::notify_owner(owner, fallback_title, &fallback_message);
                    }
                });
                self.notification_window = Some(notification);
            }
            Err(error) => {
                eprintln!("无法打开 Windows 桌面通知：{error:#}");
                if let Some(shell) = &self.shell {
                    shell.notify(title, &fallback_message);
                }
            }
        }
    }

    fn receive_notice_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.receive.notice.as_ref()?;
        if notice.kind == crate::receiving::NoticeKind::Success {
            return None;
        }
        let id = notice.id;
        let color = match notice.kind {
            crate::receiving::NoticeKind::Info => MUTED,
            crate::receiving::NoticeKind::Success => 0x30d158,
            crate::receiving::NoticeKind::Error => 0xffb56b,
        };
        Some(
            crate::share_layout::notice_overlay(
                crate::share_layout::NoticeContent {
                    id,
                    message: notice.message.clone(),
                    full_text: notice.full_text.clone(),
                    color,
                    expanded: notice.details_visible,
                    copied: notice.copied,
                    copy_label: self.language.strings().copy,
                    copied_label: self.language.strings().copied,
                },
                self.notification_position,
                cx.listener(move |this, hovered, _, cx| {
                    if this.receive.set_notice_hovered(
                        id,
                        crate::receiving::NoticeArea::Banner,
                        *hovered,
                    ) {
                        cx.notify();
                    }
                }),
                cx.listener(move |this, hovered, _, cx| {
                    if this.receive.set_notice_hovered(
                        id,
                        crate::receiving::NoticeArea::Details,
                        *hovered,
                    ) {
                        cx.notify();
                    }
                }),
                cx.listener(move |this, _, _, cx| {
                    if let Some(text) = this.receive.copy_notice(id) {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cx.notify();
                    }
                }),
                cx.listener(move |this, _, _, cx| {
                    if this
                        .receive
                        .notice
                        .as_ref()
                        .is_some_and(|notice| notice.id == id)
                        && this.receive.dismiss_notice()
                    {
                        cx.notify();
                    }
                }),
            )
            .into_any_element(),
        )
    }

    fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        div()
            .id("settings-page")
            .flex_1()
            .min_h_0()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.language_settings_view(cx))
            .child(self.send_layout_settings_view(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        checkbox(
                            "startup-prewarm",
                            text.startup_prewarm,
                            self.startup_prewarm,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_startup_prewarm(cx);
                        })),
                    )
                    .child(
                        checkbox(
                            "desktop-notifications-enabled",
                            text.notice_enabled,
                            self.desktop_notifications_enabled,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_desktop_notifications_enabled(
                                !this.desktop_notifications_enabled,
                                cx,
                            )
                        })),
                    )
                    .child(
                        checkbox(
                            "drag-activate-enabled",
                            text.drag_activate,
                            self.drag_activate,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_drag_activate(cx);
                        })),
                    )
                    .child(div().text_size(px(13.)).child(text.notice_position_title))
                    .child(self.notification_position_select(cx))
                    .child(div().text_size(px(13.)).child(text.notice_duration_title))
                    .child(self.notification_duration_select(cx)),
            )
            .child(self.radio_settings_view(cx))
            .child(self.apple_identity_view(cx))
            .child(self.trusted_senders_view(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .p(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(13.)).child(text.history_title))
                    .child(
                        button("open-transfer-history", text.history_open, false)
                            .h(px(32.))
                            .px(px(12.))
                            .on_click(cx.listener(|this, _, _, cx| this.open_history(cx))),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        checkbox(
                            "history-delete-default",
                            text.history_delete_default,
                            self.history_delete_files,
                        )
                        .when(
                            self.history_preference_pending || self.history_deleting,
                            |control| control.opacity(0.4),
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.toggle_history_delete_default(cx)),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(text.history_delete_default_hint),
                    ),
            )
            .children(self.history_preference_error.as_ref().map(|error| {
                div()
                    .id("history-preference-error")
                    .flex_shrink_0()
                    .child(self.error_view(error))
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgba(0xffffff12))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(text.device_name),
                            )
                            .when(self.name_input.is_none(), |row| {
                                row.child(
                                    div()
                                        .id("edit-device-name")
                                        .h(px(24.))
                                        .px(px(6.))
                                        .rounded(px(6.))
                                        .flex()
                                        .items_center()
                                        .text_size(px(11.))
                                        .text_color(rgb(BLUE))
                                        .child(text.change)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgb(0x32323a)))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.edit_name(window, cx)
                                        })),
                                )
                            }),
                    )
                    .child(if let Some(input) = &self.name_input {
                        input.clone().into_any_element()
                    } else {
                        div()
                            .id("settings-device-name")
                            .w_full()
                            .max_h(px(88.))
                            .visible_scrollbar()
                            .whitespace_normal()
                            .text_size(px(17.))
                            .line_height(px(22.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.device_name.clone())
                            .into_any_element()
                    })
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(17.))
                            .text_color(rgb(MUTED))
                            .child(text.device_name_hint),
                    )
                    .when_some(self.name_input.as_ref(), |card, input| {
                        let length = input.read(cx).text().chars().count();
                        card.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(
                                            if length > crate::settings::MAX_NAME_CHARS {
                                                0xffb56b
                                            } else {
                                                MUTED
                                            },
                                        ))
                                        .child(self.language.name_char_count(
                                            length,
                                            crate::settings::MAX_NAME_CHARS,
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .id("cancel-device-name")
                                                .h(px(30.))
                                                .px(px(12.))
                                                .rounded(px(8.))
                                                .bg(rgb(0x393940))
                                                .flex()
                                                .items_center()
                                                .text_size(px(11.))
                                                .cursor_pointer()
                                                .child(text.cancel)
                                                .hover(|s| s.bg(rgb(0x484850)))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.cancel_name(window, cx)
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("save-device-name")
                                                .h(px(30.))
                                                .px(px(16.))
                                                .rounded(px(8.))
                                                .bg(rgb(BLUE))
                                                .flex()
                                                .items_center()
                                                .text_size(px(11.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .cursor_pointer()
                                                .child(text.save)
                                                .hover(|s| s.bg(rgb(0x2995ff)))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.save_name(window, cx)
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .when(self.name_saved, |view| {
                view.child(div().text_size(px(11.)).text_color(rgb(0x30d158)).child(
                    if self.receiver_status.is_ok() {
                        text.name_saved_broadcast
                    } else {
                        text.name_saved_waiting
                    },
                ))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(px(13.)).child(text.receive_airdrop))
                    .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                        match &self.receiver_status {
                            Ok(()) => text.receiver_enabled.to_owned(),
                            Err(error) => error.clone(),
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(text.receiver_requires_confirm),
                    )
                    .child(div().text_size(px(11.)).child(text.save_files_to))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .whitespace_normal()
                            .child(
                                self.download_directory
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| text.folder_unavailable.into()),
                            ),
                    )
                    .child(
                        button(
                            "choose-download-directory",
                            if self.picker_open {
                                text.choosing_saving
                            } else {
                                text.change_folder
                            },
                            false,
                        )
                        .when(!self.can_change_download_directory(), |button| {
                            button.opacity(0.4)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.choose_download_directory(cx))),
                    )
                    .child(
                        checkbox(
                            "auto-open-download-dir",
                            text.auto_open_download_dir,
                            self.auto_open_download_dir,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_auto_open_download_dir(cx);
                        })),
                    )
                    .when(self.receive.active.is_some(), |card| {
                        card.child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(text.folder_change_after_receive),
                        )
                    }),
            )
            .into_any_element()
    }

    fn history_view(&self, cx: &mut Context<Self>) -> AnyElement {
        use wayline_host::{TransferDirection, TransferOutcome};
        let text = self.language.strings();
        if let Some(deletion) = &self.history_deletion {
            return self.history_delete_view(deletion, cx);
        }
        if let Some(record) = &self.history_detail {
            return self.history_detail_view(record, cx);
        }
        let mut list = div()
            .id("history-list")
            .flex_1()
            .min_h_0()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(6.));
        if self.history.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(if self.history_loaded {
                        text.history_empty
                    } else {
                        text.history_loading
                    }),
            );
        }
        for (index, record) in self.history.iter().enumerate() {
            let direction = match record.direction {
                TransferDirection::Send => text.history_direction_send,
                TransferDirection::Receive => text.history_direction_receive,
            };
            let (outcome, color) = match record.outcome {
                TransferOutcome::Completed => (text.history_completed, 0x30d158),
                TransferOutcome::Declined => (text.history_declined, MUTED),
                TransferOutcome::Cancelled => (text.history_cancelled, MUTED),
                TransferOutcome::Failed => (text.history_failed, 0xffb56b),
            };
            let file_size_text = record
                .file_bytes
                .map(file_size)
                .unwrap_or_else(|| text.history_size_unknown.into());
            let file_overview = match record.file_names.first() {
                Some(name) if record.file_count == 1 => name.clone(),
                Some(name) => format!("{} · {name}", self.language.file_count(record.file_count)),
                None => self.language.file_count(record.file_count),
            };
            let timestamp = crate::history::format_timestamp_utc(record.finished_at_unix_ms);
            let detail_record = record.clone();
            let delete_record = record.clone();
            list = list.child(
                div()
                    .id(("history-entry", index))
                    .flex_shrink_0()
                    .rounded(px(10.))
                    .bg(rgb(SURFACE))
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("{direction} {}", record.peer)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(rgb(color))
                                    .child(outcome),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(rgb(MUTED))
                            .child(div().flex_1().min_w_0().truncate().child(file_overview))
                            .child(div().flex_shrink_0().child(file_size_text)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(timestamp),
                            )
                            .child(
                                button("history-details", text.history_details, false)
                                    .id(("history-details", index))
                                    .h(px(28.))
                                    .min_w(px(60.))
                                    .px(px(12.))
                                    .flex_shrink_0()
                                    .rounded(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.history_detail = Some(detail_record.clone());
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("history-delete", text.history_delete, false)
                                    .id(("history-delete", index))
                                    .h(px(28.))
                                    .min_w(px(60.))
                                    .px(px(12.))
                                    .flex_shrink_0()
                                    .rounded(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.request_history_deletion(
                                            vec![delete_record.clone()],
                                            cx,
                                        )
                                    })),
                            ),
                    ),
            );
        }

        let clear_controls = button("clear-history", text.history_clear, false)
            .w_full()
            .when(
                self.history.is_empty() || self.history_preference_pending,
                |button| button.opacity(0.4).cursor(CursorStyle::Arrow),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.request_history_deletion(this.history.clone(), cx)
            }));

        div()
            .id("history-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(list)
            .child(clear_controls)
            .children(
                self.history_error
                    .as_ref()
                    .map(|error| self.error_view(error)),
            )
            .children(self.history_preference_error.as_ref().map(|error| {
                div()
                    .id("history-preference-error")
                    .flex_shrink_0()
                    .child(self.error_view(error))
            }))
            .into_any_element()
    }

    fn history_detail_view(
        &self,
        record: &wayline_host::TransferHistoryRecord,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use wayline_host::{TransferDirection, TransferFailureReason, TransferOutcome};
        let text = self.language.strings();
        let direction = match record.direction {
            TransferDirection::Send => text.history_direction_send,
            TransferDirection::Receive => text.history_direction_receive,
        };
        let (outcome, color) = match record.outcome {
            TransferOutcome::Completed => (text.history_completed, 0x30d158),
            TransferOutcome::Declined => (text.history_declined, MUTED),
            TransferOutcome::Cancelled => (text.history_cancelled, MUTED),
            TransferOutcome::Failed => (text.history_failed, 0xffb56b),
        };
        let failure = match record.failure_reason {
            Some(TransferFailureReason::Connection) => Some(text.history_failure_connection),
            Some(TransferFailureReason::Validation) => Some(text.history_failure_validation),
            Some(TransferFailureReason::Other) => Some(text.history_failure_other),
            None => None,
        };
        let selected = record.clone();
        let mut files = div()
            .id("history-detail-files")
            .flex_1()
            .min_h_0()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(8.));
        files = files
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{direction} {}", record.peer)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .child(outcome),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(crate::history::format_timestamp_utc(
                        record.finished_at_unix_ms,
                    )),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(
                        self.language
                            .transfer_network(record.channel, record.format),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .line_height(px(17.))
                    .whitespace_normal()
                    .text_color(rgb(MUTED))
                    .child(
                        self.language.history_summary(
                            record.file_count,
                            &record
                                .file_bytes
                                .map(file_size)
                                .unwrap_or_else(|| text.history_size_unknown.into()),
                            &file_size(record.transferred_bytes),
                            &self.language.history_duration(record.duration_ms),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(
                        self.language.history_average_speed(
                            &crate::history::average_bytes_per_second(record)
                                .map(file_size)
                                .unwrap_or_else(|| "—".into()),
                        ),
                    ),
            )
            .when_some(failure, |details, failure| {
                details.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(rgb(0xffb56b))
                        .whitespace_normal()
                        .child(failure),
                )
            });
        // New records render each file name together with its location and actions
        // below. Keep the standalone names only for legacy records without paths.
        if record.file_paths.is_empty() {
            if record.file_names.is_empty() {
                files = files.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(text.history_files_unknown),
                );
            } else {
                for name in &record.file_names {
                    files = files.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .whitespace_normal()
                            .child(name.clone()),
                    );
                }
            }
        }
        for (index, path) in record.file_paths.iter().enumerate() {
            let file = path.clone();
            let location = path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| path.clone());
            let file_name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
            files = files.child(
                div()
                    .flex_shrink_0()
                    .rounded(px(10.))
                    .bg(rgb(SURFACE))
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_normal()
                            .child(file_name),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(location.display().to_string()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .gap(px(8.))
                            .child(
                                button("history-file-open", text.open_file, false)
                                    .id(("history-file-open", index))
                                    .h(px(28.))
                                    .min_w(px(72.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_history_file(file.clone(), cx)
                                    })),
                            )
                            .child(
                                button("history-file-location", text.history_open_location, false)
                                    .id(("history-file-location", index))
                                    .h(px(28.))
                                    .min_w(px(72.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_history_folder(location.clone(), cx)
                                    })),
                            ),
                    ),
            );
        }
        // Older records only retain the receive folder, so keep that location usable.
        if record.file_paths.is_empty()
            && let Some(destination) = &record.destination
        {
            let destination = destination.clone();
            files = files.child(
                button("history-old-location", text.history_open_location, false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.open_history_folder(destination.clone(), cx)
                    }),
                ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(files)
            .child(
                button("history-detail-delete", text.history_delete, false).on_click(cx.listener(
                    move |this, _, _, cx| this.request_history_deletion(vec![selected.clone()], cx),
                )),
            )
            .children(
                self.history_error
                    .as_ref()
                    .map(|error| self.error_view(error)),
            )
            .children(self.history_preference_error.as_ref().map(|error| {
                div()
                    .id("history-preference-error")
                    .flex_shrink_0()
                    .child(self.error_view(error))
            }))
            .into_any_element()
    }

    fn history_delete_view(
        &self,
        deletion: &HistoryDeletion,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.language.strings();
        let body = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text.history_delete_prompt),
            )
            .child(
                div()
                    .id("delete-history-items")
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(deletion.records.iter().map(|record| {
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .whitespace_normal()
                            .child(format!(
                                "{} · {}",
                                record.peer,
                                if record.file_names.is_empty() {
                                    self.language.file_count(record.file_count)
                                } else {
                                    record
                                        .file_names
                                        .iter()
                                        .take(3)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                }
                            ))
                    })),
            )
            .child(
                checkbox(
                    "delete-history-files",
                    text.history_delete_files,
                    deletion.delete_files,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.history_deleting
                        && let Some(deletion) = &mut this.history_deletion
                    {
                        deletion.delete_files = !deletion.delete_files;
                        cx.notify();
                    }
                })),
            )
            .when(deletion.delete_files, |view| {
                view.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(rgb(0xffb56b))
                        .whitespace_normal()
                        .child(text.history_delete_files_hint),
                )
            })
            .when(
                deletion.delete_files
                    && deletion
                        .records
                        .iter()
                        .any(|record| record.file_paths.is_empty()),
                |view| {
                    view.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .whitespace_normal()
                            .child(text.history_delete_unavailable),
                    )
                },
            )
            .child(
                checkbox(
                    "remember-history-deletion",
                    text.history_delete_remember,
                    deletion.remember,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.history_deleting
                        && let Some(deletion) = &mut this.history_deletion
                    {
                        deletion.remember = !deletion.remember;
                        cx.notify();
                    }
                })),
            )
            .children(
                self.history_error
                    .as_ref()
                    .map(|error| self.error_view(error)),
            )
            .children(self.history_preference_error.as_ref().map(|error| {
                div()
                    .id("history-preference-error")
                    .flex_shrink_0()
                    .child(self.error_view(error))
            }))
            .into_any_element();
        let actions = div()
            .flex()
            .gap(px(8.))
            .child(
                button(
                    "confirm-delete-history",
                    if self.history_deleting {
                        text.history_delete_pending
                    } else {
                        text.history_delete_confirm
                    },
                    true,
                )
                .flex_1()
                .when(self.history_deleting, |button| button.opacity(0.4))
                .on_click(cx.listener(|this, _, _, cx| this.confirm_history_deletion(cx))),
            )
            .child(
                button("cancel-delete-history", text.cancel, false)
                    .flex_1()
                    .when(self.history_deleting, |button| button.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_history_deletion(cx))),
            );
        crate::share_layout::page_with_actions(
            "history-delete-confirmation",
            body,
            actions.into_any_element(),
        )
        .into_any_element()
    }

    fn trusted_senders_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.language.strings();
        div()
            .flex_shrink_0()
            .rounded(px(16.))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_size(px(13.)).child(text.trusted_senders))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(text.trusted_senders_hint),
            )
            .children(
                self.trusted_senders
                    .iter()
                    .enumerate()
                    .map(|(index, sender)| {
                        let pin = sender.certificate_sha256;
                        let open_web_links = sender.open_web_links;
                        let row = div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.))
                                            .child(sender.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(MUTED))
                                            .child(short_certificate_pin(pin)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("forget-trusted-sender", index))
                                    .flex_shrink_0()
                                    .px(px(7.))
                                    .py(px(5.))
                                    .rounded(px(6.))
                                    .text_size(px(11.))
                                    .text_color(rgb(BLUE))
                                    .child(text.trusted_sender_forget)
                                    .cursor_pointer()
                                    .hover(|item| item.bg(rgb(0x38383d)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_trusted_senders(
                                            wayline_host::forget_trusted_sender(pin),
                                        );
                                        cx.notify();
                                    })),
                            );
                        div().flex().flex_col().gap(px(5.)).child(row).child(
                            checkbox(
                                ("trusted-open-links", index),
                                text.trusted_sender_open_links,
                                open_web_links,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.update_trusted_senders(
                                        wayline_host::set_trusted_sender_open_web_links(
                                            pin,
                                            !open_web_links,
                                        ),
                                    );
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            )
    }

    fn update_trusted_senders(&mut self, result: anyhow::Result<Vec<wayline_host::TrustedSender>>) {
        match result {
            Ok(senders) => self.trusted_senders = senders,
            Err(error) => {
                let message = format!(
                    "{} {error:#}",
                    self.language.strings().trusted_senders_error
                );
                self.settings_error = Some(message.clone());
                self.receive
                    .show_notice(message, crate::receiving::NoticeKind::Error);
            }
        }
    }

    fn answer_incoming(&mut self, answer: IncomingAnswer, cx: &mut Context<Self>) {
        let trust_incoming = std::mem::take(&mut self.trust_incoming);
        if let Some(request) = self.incoming.take() {
            let active = request.active();
            let accepted = !matches!(answer, IncomingAnswer::Decline) && active;
            let channel = self.model.current_channel();
            let declined_record = (!accepted && active).then(|| {
                let mut record = crate::receiving::declined_history_record(
                    &request,
                    crate::history::unix_time_ms(std::time::SystemTime::now()),
                    Instant::now(),
                );
                record.channel = channel;
                record
            });
            self.receiver_declined_until = None;
            request
                .accepting
                .store(accepted, std::sync::atomic::Ordering::Release);
            let delivered = request.decision.send(accepted).is_ok();
            if accepted && delivered {
                self.receive.set_channel(channel);
                self.receive.begin(&request.transfer, request.save_dir);
                if matches!(answer, IncomingAnswer::AutoAccept) {
                    self.receive.mark_automatic_acceptance();
                }
                if matches!(answer, IncomingAnswer::Accept) {
                    self.settings_open = false;
                    self.pending_send_layout = None;
                    self.firmware.open = false;
                }
                if trust_incoming
                    && let Some(certificate_sha256) = request.transfer.file_offer_certificate()
                {
                    self.update_trusted_senders(wayline_host::trust_sender(
                        wayline_host::TrustedSender {
                            name: request.transfer.sender_name,
                            certificate_sha256,
                            open_web_links: false,
                        },
                    ));
                }
            } else if active && delivered {
                self.receive.dismiss_notice();
                self.receiver_declined_until = Some(Instant::now() + Duration::from_secs(3));
            } else {
                self.receive.show_notice(
                    self.language.strings().incoming_ended,
                    crate::receiving::NoticeKind::Info,
                );
            }
            if let Some(record) = declined_record
                && delivered
                && let Some(worker) = &self.history_worker
            {
                worker.append(record, Vec::new());
            }
            cx.notify();
        }
    }

    fn open_received_web_links(urls: Vec<String>, cx: &mut Context<Self>) {
        if !urls.is_empty() {
            crate::receiving::log(&format!(
                "opening {} accepted web links in the default browser",
                urls.len()
            ));
            for url in urls {
                cx.open_url(&url);
            }
        }
    }

    fn incoming_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let request = self.incoming.as_ref().unwrap();
        let is_link_share = !request.transfer.urls.is_empty();
        let has_files = !request.transfer.files.is_empty();
        let share_text = request.transfer.share_text.as_ref();
        let certificate = request
            .transfer
            .file_offer_certificate()
            .filter(|_| wayline_host::normalize_name(&request.transfer.sender_name).is_ok());
        let content = div()
            .flex_grow()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if is_link_share && has_files {
                        text.receive_files_and_links_title
                    } else if is_link_share {
                        self.language.shared_links()
                    } else if !has_files && share_text.is_some() {
                        text.receive_shared_text
                    } else {
                        text.receive_files_title
                    }),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .whitespace_normal()
                    .child(if !has_files {
                        request.transfer.sender_name.clone()
                    } else {
                        self.language.incoming_ask(
                            &request.transfer.sender_name,
                            request.transfer.files.len(),
                        )
                    }),
            )
            .child(
                div().text_size(px(12.)).text_color(rgb(MUTED)).children(
                    request
                        .transfer
                        .files
                        .iter()
                        .map(|file| file.name.clone())
                        .chain(
                            request
                                .transfer
                                .urls
                                .iter()
                                .filter_map(|url| wayline_core::airdrop::web_url(url)),
                        )
                        .map(|name| div().whitespace_normal().mb(px(6.)).child(name)),
                ),
            )
            .when_some(share_text, |view, original| {
                view.child(
                    div()
                        .id("incoming-share-text")
                        .max_h(px(100.))
                        .visible_scrollbar()
                        .whitespace_normal()
                        .text_size(px(12.))
                        .child(original.clone()),
                )
            })
            .when(has_files, |view| {
                view.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .whitespace_normal()
                        .child(
                            self.language
                                .accepted_save_to(&request.save_dir.display().to_string()),
                        ),
                )
            });
        let actions = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when_some(certificate, |actions, pin| {
                actions
                    .child(
                        checkbox(
                            "trust-incoming",
                            text.trusted_sender_auto_accept,
                            self.trust_incoming,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.trust_incoming = !this.trust_incoming;
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .pl(px(25.))
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child(short_certificate_pin(pin)),
                    )
            })
            .child(
                button(
                    "accept-incoming",
                    if is_link_share && has_files {
                        text.accept_files_and_open_links
                    } else if is_link_share {
                        self.language.open_shared_links()
                    } else {
                        text.accept
                    },
                    true,
                )
                .on_click(
                    cx.listener(|this, _, _, cx| this.answer_incoming(IncomingAnswer::Accept, cx)),
                ),
            )
            .child(
                button("decline-incoming", text.decline, true)
                    .bg(rgb(SURFACE))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.answer_incoming(IncomingAnswer::Decline, cx)
                    })),
            );
        crate::share_layout::page_with_actions(
            "incoming-request",
            content.into_any_element(),
            actions.into_any_element(),
        )
        .into_any_element()
    }

    fn send_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let (ready, empty) = (
            self.model.phase == Phase::Ready,
            self.model.files.is_empty(),
        );
        let page = if self.send_layout != SendLayout::Classic {
            self.compact_send_panel(cx)
        } else if !ready {
            self.transfer_view(cx)
        } else if empty {
            self.empty_view(cx)
        } else {
            self.files_view(cx)
        };
        if !self.animate_send_view {
            return page;
        }
        // One identity per screen. Device discoveries and progress updates
        // repaint in place and do not restart the entrance animation.
        let animation_id = if !ready {
            "send-transfer-entrance"
        } else if empty {
            "send-empty-entrance"
        } else {
            "send-devices-entrance"
        };
        div()
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(page)
            .with_animation(
                animation_id,
                Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                |page, progress| {
                    page.opacity(0.7 + progress * 0.3)
                        .top(px(3. * (1. - progress)))
                },
            )
            .into_any_element()
    }

    fn empty_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let illustration = div()
            .my_auto()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .child(self.orb.element())
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(23.))
                    .line_height(px(30.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text.share_tagline),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(rgb(MUTED))
                    .text_center()
                    .child(text.share_hint),
            )
            .children(
                self.model
                    .error
                    .as_ref()
                    .map(|error| self.error_view(error)),
            );
        let action = button(
            "choose",
            if self.picker_open {
                text.opening_picker
            } else if self.model.validating {
                text.footer_reading
            } else {
                text.choose_files
            },
            !self.picker_open && !self.model.validating,
        )
        .mt(px(8.))
        .flex_shrink_0()
        .w_full()
        .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx)));
        crate::share_layout::empty_share(illustration.into_any_element(), action.into_any_element())
            .into_any_element()
    }

    fn file_previews(&self) -> AnyElement {
        let count = self.model.files.len().min(MAX_PREVIEWS);
        if count == 1 {
            return self.file_tile(0, 44.);
        }
        div()
            .relative()
            .size(px(46.))
            .flex_shrink_0()
            .children((0..count).map(|index| {
                div()
                    .absolute()
                    .left(px((index % 2) as f32 * 24.))
                    .top(px(if count == 2 {
                        12.
                    } else {
                        (index / 2) as f32 * 24.
                    }))
                    .child(self.file_tile(index, 22.))
            }))
            .into_any_element()
    }

    fn file_tile(&self, index: usize, side: f32) -> AnyElement {
        let kind = FileKind::for_path(&self.model.files[index]);
        let tile = div()
            .size(px(side))
            .flex_shrink_0()
            .rounded(px(side * 0.18))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center();
        if let Some(Some(image)) = self.model.previews.get(index) {
            return tile
                .bg(rgb(0x323238))
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element();
        }
        // A recognizable document badge is present immediately, including
        // when the associated shell extension cannot produce an icon.
        let (label, color) = kind.badge(self.language);
        tile.bg(rgb(color))
            .child(
                div()
                    .w(px(side * 0.66))
                    .h(px(side * 0.78))
                    .rounded(px(2.))
                    .bg(rgb(0xf5f5f7))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(side * 0.07))
                    .text_color(rgb(color))
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(if label.len() == 1 {
                        side * 0.42
                    } else {
                        side * 0.20
                    }))
                    .child(label)
                    .child(div().w(px(side * 0.36)).h(px(1.)).bg(rgb(color)))
                    .child(div().w(px(side * 0.36)).h(px(1.)).bg(rgb(color))),
            )
            .into_any_element()
    }

    fn files_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let title = if self.model.files.len() == 1 {
            self.model.files[0]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            self.language.file_count(self.model.files.len())
        };
        let mut devices = div()
            .id("devices")
            .flex_1()
            .min_h_0()
            .visible_scrollbar()
            .flex()
            .flex_col()
            .gap(px(7.));
        if self.model.peers.is_empty() {
            devices = devices.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap(px(9.))
                    .text_color(rgb(MUTED))
                    .text_size(px(12.))
                    .child(
                        if self.model.status.is_some() && !self.model.connection_ready() {
                            text.connection_not_ready
                        } else if self.model.scanning {
                            text.scanning
                        } else {
                            text.no_receivers
                        },
                    )
                    .child(div().text_size(px(10.)).text_center().child(
                        if self.model.status.is_some() && !self.model.connection_ready() {
                            text.reconnect_search
                        } else {
                            text.device_awake_hint
                        },
                    )),
            );
        }
        for (index, peer) in self.model.peers.iter().enumerate() {
            let selected = self.model.selected.as_ref() == Some(&peer.service.instance);
            let instance = peer.service.instance.clone();
            devices = devices.child(
                div()
                    .id(("peer", index))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .p(px(12.))
                    .rounded(px(16.))
                    .border_1()
                    .border_color(if selected {
                        rgb(BLUE)
                    } else {
                        rgba(0xffffff12)
                    })
                    .bg(if selected {
                        rgb(0x173353)
                    } else {
                        rgb(SURFACE)
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.toggle_peer(&instance);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(32.))
                            .rounded(px(5.))
                            .border_2()
                            .border_color(rgb(if selected { BLUE } else { MUTED }))
                            .flex()
                            .justify_center()
                            .items_end()
                            .pb(px(3.))
                            .child(div().w(px(8.)).h(px(2.)).rounded_full().bg(rgb(MUTED))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().truncate().text_size(px(13.)).child(peer.name.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .child(format!(
                                        "AirDrop · {}",
                                        peer.service.instance.split('.').next().unwrap_or("")
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_color(rgb(BLUE))
                            .child(if selected { "✓" } else { "" }),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_start()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(16.))
                    .bg(rgb(SURFACE))
                    .child(self.file_previews())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .id("file-name")
                                    .w_full()
                                    .max_h(px(96.))
                                    .visible_scrollbar()
                                    .whitespace_normal()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.))
                                    .text_size(px(10.))
                                    .child(
                                        div()
                                            .text_color(rgb(MUTED))
                                            .child(file_size(self.model.bytes)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_shrink_0()
                                            .items_center()
                                            .gap(px(12.))
                                            .child(
                                                div()
                                                    .id("open-file")
                                                    .text_color(rgb(if self.can_open_files() {
                                                        BLUE
                                                    } else {
                                                        MUTED
                                                    }))
                                                    .when(self.can_open_files(), |button| {
                                                        button.cursor_pointer()
                                                    })
                                                    .child(if self.opening_files.is_some() {
                                                        text.open_in_progress
                                                    } else if self.model.files.len() > 1 {
                                                        text.open_all
                                                    } else {
                                                        text.open_file
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_files(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("replace")
                                                    .cursor_pointer()
                                                    .text_color(rgb(BLUE))
                                                    .child(if self.picker_open {
                                                        text.selecting
                                                    } else if self.model.validating {
                                                        text.reading_short
                                                    } else {
                                                        text.replace_files
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.choose(window, cx)
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(text.send_to_nearby)
                    .child(
                        div()
                            .id("refresh")
                            .cursor_pointer()
                            .text_color(rgb(BLUE))
                            .child(if self.model.scanning {
                                if self.model.peers.is_empty() {
                                    text.searching
                                } else {
                                    text.searching_more
                                }
                            } else {
                                text.search_again
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
                    ),
            )
            .child(devices)
            .children(
                self.model
                    .error
                    .as_ref()
                    .map(|error| self.error_view(error)),
            )
            .child(
                button(
                    "send",
                    if self.model.selected.is_some() {
                        text.send
                    } else {
                        text.choose_device
                    },
                    self.model.can_send(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
            )
            .into_any_element()
    }

    fn transfer_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let complete = self.model.phase == Phase::Complete;
        let mut view = div().w_full().flex_1().min_h_0().flex().flex_col().child(
            div()
                .id("send-readout-scroll")
                .flex_1()
                .min_h_0()
                .visible_scrollbar()
                .child(self.send_readout()),
        );
        view = view.children(
            self.model
                .error
                .as_ref()
                .map(|error| self.error_view(error)),
        );
        if self.model.phase.busy() {
            view = view.child(
                button(
                    "cancel",
                    if self.model.phase == Phase::Cancelling {
                        text.cancelling
                    } else {
                        text.cancel_send
                    },
                    false,
                )
                .w_full()
                .flex_shrink_0()
                .mt(px(12.))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.model.phase != Phase::Cancelling {
                        this.cancel(cx);
                    }
                })),
            );
        } else {
            view = view.child(
                button(
                    "done",
                    if complete {
                        text.done
                    } else {
                        text.back_action
                    },
                    true,
                )
                .w_full()
                .flex_shrink_0()
                .mt(px(12.))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.model.phase == Phase::Complete {
                        this.reset(window, cx);
                    } else {
                        this.model.phase = Phase::Ready;
                        this.model.error = None;
                        cx.notify();
                    }
                })),
            );
        }
        view.into_any_element()
    }
    fn error_view(&self, error: &str) -> AnyElement {
        div()
            .id("error")
            .flex_shrink_0()
            .w_full()
            .max_h(px(65.))
            .visible_scrollbar()
            .text_size(px(10.))
            .whitespace_normal()
            .text_color(rgb(0xffb56b))
            .child(error.to_string())
            .into_any_element()
    }
}
impl Drop for Capsule {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &self.scan_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &self.firmware_cancel {
            cancel.cancel();
        }
    }
}
impl Render for Capsule {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_orb = self.visibility == Visibility::Share
            && self.send_layout == SendLayout::Classic
            && self.share_result.is_none()
            && self.exit_state.is_none()
            && self.incoming.is_none()
            && self.receive.active.is_none()
            && !self.firmware.open
            && !self.settings_open
            && self.model.phase == Phase::Ready
            && self.model.files.is_empty();
        self.orb.update(show_orb, self.model.connection_ready());
        div()
            .id("root")
            .size_full()
            .font_family(self.language.font_family())
            .text_color(rgb(INK))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.exit_state.is_some() {
                    return;
                }
                if event.keystroke.modifiers.control && event.keystroke.key == "q" {
                    this.quit(window, cx);
                    return;
                }
                if this.language_menu_open && event.keystroke.key == "escape" {
                    this.language_menu_open = false;
                    cx.notify();
                    return;
                }
                if this.notification_position_menu_open || this.notification_duration_menu_open {
                    if event.keystroke.key == "escape" {
                        this.notification_position_menu_open = false;
                        this.notification_duration_menu_open = false;
                        cx.notify();
                    }
                    return;
                }
                if this.name_input.is_some() {
                    return;
                }
                if event.keystroke.key == "escape" {
                    if this.settings_open || this.firmware.open {
                        this.back(window, cx);
                    } else {
                        this.expand(false, window, cx);
                    }
                }
                if !this.settings_open
                    && event.keystroke.modifiers.control
                    && event.keystroke.key == "o"
                {
                    this.choose(window, cx);
                }
            }))
            .child(if self.exit_state.is_some() {
                self.exit_panel(cx)
            } else if self.visibility == Visibility::Share {
                self.panel(cx)
            } else {
                self.pill(cx)
            })
    }
}
struct IconTooltip(&'static str);

impl Render for IconTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(9.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(rgb(0x393940))
            .text_size(px(11.))
            .text_color(rgb(INK))
            .child(self.0)
    }
}

fn icon_tooltip(label: &'static str, _: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| IconTooltip(label)).into()
}

fn short_certificate_pin(pin: [u8; 32]) -> String {
    let prefix: String = pin[..6].iter().map(|byte| format!("{byte:02X}")).collect();
    format!("SHA-256 {prefix}…")
}

fn checkbox(id: impl Into<ElementId>, label: &'static str, checked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .child(
            div()
                .size(px(17.))
                .flex_shrink_0()
                .rounded(px(4.))
                .border_1()
                .border_color(rgb(if checked { BLUE } else { MUTED }))
                .bg(rgb(if checked { BLUE } else { SURFACE }))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .child(if checked { "✓" } else { "" }),
        )
        .child(div().text_size(px(11.)).whitespace_normal().child(label))
}

fn firmware_icon() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let at = |x, y| bounds.origin + point(px(x), px(y));
            let mut chip = PathBuilder::stroke(px(1.25));
            chip.move_to(at(4., 4.));
            chip.line_to(at(12., 4.));
            chip.line_to(at(12., 12.));
            chip.line_to(at(4., 12.));
            chip.close();
            for pin in [5., 8., 11.] {
                for (from, to) in [
                    ((pin, 1.5), (pin, 4.)),
                    ((pin, 12.), (pin, 14.5)),
                    ((1.5, pin), (4., pin)),
                    ((12., pin), (14.5, pin)),
                ] {
                    chip.move_to(at(from.0, from.1));
                    chip.line_to(at(to.0, to.1));
                }
            }
            if let Ok(path) = chip.build() {
                window.paint_path(path, window.text_style().color);
            }
        },
    )
    .size(px(16.))
    .flex_shrink_0()
}

fn history_icon() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let at = |x, y| bounds.origin + point(px(x), px(y));
            let mut clock = PathBuilder::stroke(px(1.25));
            for index in 0..=32 {
                let angle =
                    -std::f32::consts::PI * 0.8 + index as f32 * std::f32::consts::TAU * 0.88 / 32.;
                let position = at(8. + angle.cos() * 6., 8. + angle.sin() * 6.);
                if index == 0 {
                    clock.move_to(position);
                } else {
                    clock.line_to(position);
                }
            }
            clock.move_to(at(1.5, 2.));
            clock.line_to(at(1.5, 6.));
            clock.line_to(at(5.5, 6.));
            clock.move_to(at(8., 4.5));
            clock.line_to(at(8., 8.));
            clock.line_to(at(10.5, 9.5));
            if let Ok(path) = clock.build() {
                window.paint_path(path, window.text_style().color);
            }
        },
    )
    .size(px(16.))
    .flex_shrink_0()
}

fn settings_gear() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let center = bounds.origin + point(px(8.), px(8.));
            let mut gear = PathBuilder::stroke(px(1.25));
            for tooth in 0..8 {
                let angle = tooth as f32 * std::f32::consts::TAU / 8.;
                for (index, (offset, radius)) in
                    [(-0.16, 5.1), (-0.16, 6.6), (0.16, 6.6), (0.16, 5.1)]
                        .into_iter()
                        .enumerate()
                {
                    let angle = angle + offset;
                    let position =
                        center + point(px(angle.cos() * radius), px(angle.sin() * radius));
                    if tooth == 0 && index == 0 {
                        gear.move_to(position);
                    } else {
                        gear.line_to(position);
                    }
                }
            }
            gear.close();
            for index in 0..24 {
                let angle = index as f32 * std::f32::consts::TAU / 24.;
                let position = center + point(px(angle.cos() * 2.2), px(angle.sin() * 2.2));
                if index == 0 {
                    gear.move_to(position);
                } else {
                    gear.line_to(position);
                }
            }
            gear.close();
            if let Ok(path) = gear.build() {
                window.paint_path(path, window.text_style().color);
            }
        },
    )
    .size(px(16.))
    .flex_shrink_0()
}

fn dropdown_chevron(open: bool) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            let mut path = PathBuilder::stroke(px(1.25));
            let (start_y, middle_y, end_y) = if open { (6., 3., 6.) } else { (2., 5., 2.) };
            path.move_to(bounds.origin + point(px(2.), px(start_y)));
            path.line_to(bounds.origin + point(px(5.), px(middle_y)));
            path.line_to(bounds.origin + point(px(8.), px(end_y)));
            if let Ok(path) = path.build() {
                window.paint_path(path, window.text_style().color);
            }
        },
    )
    .size(px(10.))
    .flex_shrink_0()
}

fn back_chevron() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(bounds.origin + point(px(8.5), px(3.)));
            path.line_to(bounds.origin + point(px(4.5), px(7.)));
            path.line_to(bounds.origin + point(px(8.5), px(11.)));
            if let Ok(path) = path.build() {
                let color = window.text_style().color;
                window.paint_path(path, color);
            }
        },
    )
    .size(px(14.))
    .flex_shrink_0()
}

fn pin_icon(pinned: bool) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |x, y| bounds.origin + point(px(x), px(y));
            let color = window.text_style().color;
            let mut pin = PathBuilder::stroke(px(1.25));
            if pinned {
                pin.move_to(at(5.0, 2.5));
                pin.line_to(at(11.0, 2.5));
                pin.move_to(at(6.5, 2.5));
                pin.line_to(at(6.5, 5.0));
                pin.move_to(at(9.5, 2.5));
                pin.line_to(at(9.5, 5.0));
                pin.line_to(at(12.0, 8.5));
                pin.line_to(at(4.0, 8.5));
                pin.line_to(at(6.5, 5.0));
                pin.move_to(at(3.5, 8.5));
                pin.line_to(at(12.5, 8.5));
                pin.move_to(at(8.0, 8.5));
                pin.line_to(at(8.0, 14.0));
            } else {
                pin.move_to(at(8.5, 1.5));
                pin.line_to(at(14.5, 7.5));
                pin.move_to(at(10.0, 3.0));
                pin.line_to(at(8.5, 4.5));
                pin.move_to(at(13.0, 6.0));
                pin.line_to(at(11.5, 7.5));
                pin.line_to(at(11.0, 11.0));
                pin.line_to(at(5.0, 5.0));
                pin.line_to(at(8.5, 4.5));
                pin.move_to(at(5.0, 5.0));
                pin.line_to(at(11.0, 11.0));
                pin.move_to(at(8.0, 8.0));
                pin.line_to(at(3.5, 12.5));
            }
            if let Ok(path) = pin.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(16.))
    .flex_shrink_0()
}

fn firmware_card() -> Div {
    div()
        .w_full()
        .flex_shrink_0()
        .rounded(px(12.))
        .border_1()
        .border_color(rgba(0xffffff12))
        .bg(rgb(SURFACE))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
}

fn firmware_hint(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(rgb(color))
        .whitespace_normal()
        .child(text.into())
}

fn firmware_button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    button(id, label, primary)
        .h(px(36.))
        .w_full()
        .flex_shrink_0()
        .rounded(px(10.))
        .text_size(px(12.))
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(42.))
        .rounded(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .bg(rgb(if primary { BLUE } else { 0x303037 }))
        .text_color(rgb(if primary { 0xffffff } else { MUTED }))
        .child(label)
        .hover(move |s| s.bg(rgb(if primary { 0x2592ff } else { 0x3b3b43 })))
}
