use super::*;

impl Capsule {
    pub(super) fn apple_identity_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.language.strings();
        let available = self.can_edit_apple_identity();
        div()
            .flex_shrink_0()
            .rounded(px(16.))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_size(px(13.)).child(text.apple_identity_title))
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                if self.apple_identity_imported {
                    text.apple_identity_imported
                } else {
                    text.apple_identity_hint
                },
            ))
            .when(self.apple_identity_imported, |card| {
                card.child(
                    checkbox(
                        "use-apple-identity",
                        text.apple_identity_use,
                        self.use_apple_identity,
                    )
                    .when(available, |toggle| {
                        toggle.on_click(cx.listener(|this, _, _, cx| {
                            if this.can_edit_apple_identity() && this.apple_identity_imported {
                                this.set_apple_identity_enabled(!this.use_apple_identity);
                                cx.notify();
                            }
                        }))
                    }),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .opacity(if available { 1. } else { 0.45 })
                    .child(
                        button("import-apple-identity", text.apple_identity_import, false)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| this.import_apple_identity(cx))),
                    )
                    .when(self.apple_identity_imported, |row| {
                        row.child(
                            button("remove-apple-identity", text.apple_identity_remove, false)
                                .flex_1()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.remove_apple_identity(cx)),
                                ),
                        )
                    }),
            )
    }

    fn can_edit_apple_identity(&self) -> bool {
        !self.picker_open
            && !self.model.scanning
            && !self.model.phase.busy()
            && !self.radio_change_pending
            && self.exit_state.is_none()
    }

    fn set_apple_identity_enabled(&mut self, enabled: bool) {
        self.use_apple_identity = enabled;
        if let Some(cancel) = self.scan_cancel.take() {
            cancel.cancel();
        }
        self.model.scan_id += 1;
        self.model.scanning = false;
        self.model.peers.clear();
        self.model.selected = None;
    }

    fn import_apple_identity(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_apple_identity() {
            return;
        }
        let Some(shell) = &self.shell else {
            return;
        };
        match shell.choose_apple_identity_directory() {
            Ok(()) => {
                self.settings_error = None;
                self.picker_open = true;
            }
            Err(error) => self.apple_identity_updated(Err(format!("{error:#}"))),
        }
        cx.notify();
    }

    pub(super) fn apple_identity_directory_chosen(
        &mut self,
        result: Result<Option<PathBuf>, String>,
    ) {
        match result {
            Ok(Some(directory)) => {
                let tx = self.tx.clone();
                self.workers.push(std::thread::spawn(move || {
                    let result = wayline_host::import_apple_airdrop_identity(&directory)
                        .map(|()| Some(true))
                        .map_err(|error| format!("{error:#}"));
                    let _ = tx.send(Event::AppleIdentityUpdated(result));
                }));
            }
            Ok(None) => self.apple_identity_updated(Ok(None)),
            Err(error) => self.apple_identity_updated(Err(error)),
        }
    }

    fn remove_apple_identity(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_apple_identity() || !self.apple_identity_imported {
            return;
        }
        self.picker_open = true;
        self.settings_error = None;
        let tx = self.tx.clone();
        self.workers.push(std::thread::spawn(move || {
            let result = wayline_host::remove_apple_airdrop_identity()
                .map(|()| Some(false))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::AppleIdentityUpdated(result));
        }));
        cx.notify();
    }

    pub(super) fn apple_identity_updated(&mut self, result: Result<Option<bool>, String>) {
        self.picker_open = false;
        match result {
            Ok(Some(imported)) => {
                self.apple_identity_imported = imported;
                self.set_apple_identity_enabled(false);
                self.settings_error = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.settings_error = Some(format!(
                    "{} {error}",
                    self.language.strings().apple_identity_error
                ));
            }
        }
    }
}
