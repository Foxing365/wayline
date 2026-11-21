use super::{Capsule, MUTED, Phase, SURFACE, button, file_size};
use crate::scroll_area::ScrollbarExt;
use gpui::prelude::FluentBuilder;
use gpui::*;

const SEND_SHEET: u32 = 0xeef2f7;
const SEND_INK: u32 = 0x172538;
const SEND_MUTED: u32 = 0x4d5d74;
const SEND_ACCENT: u32 = 0x205da9;

impl Capsule {
    pub(super) fn send_layout_settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        div()
            .flex_shrink_0()
            .rounded(px(16.))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_size(px(13.)).child(text.send_layout_title))
            .child(
                div().flex().gap(px(6.)).children(
                    [
                        ("classic-send-layout", text.send_layout_classic, false),
                        ("compact-send-layout", text.send_layout_compact, true),
                    ]
                    .map(|(id, label, compact)| {
                        button(id, label, self.compact_send_view == compact)
                            .h(px(32.))
                            .min_w_0()
                            .flex_1()
                            .rounded(px(8.))
                            .text_size(px(11.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_compact_send_view(compact, cx);
                            }))
                    }),
                ),
            )
            .into_any_element()
    }

    pub(super) fn send_layout_refresh_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let layout = if self.pending_send_layout == Some(true) {
            text.send_layout_compact
        } else {
            text.send_layout_classic
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .justify_center()
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(14.))
                    .bg(rgb(SURFACE))
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(layout),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .whitespace_normal()
                            .child(text.send_layout_refresh_hint),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                button("cancel-send-layout-refresh", text.cancel, false)
                                    .flex_1()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_send_layout_refresh(cx)
                                    })),
                            )
                            .child(
                                button("apply-send-layout-refresh", text.send_layout_refresh, true)
                                    .flex_1()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.refresh_send_layout(window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn compact_send_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        if self.model.phase != Phase::Ready {
            return self.compact_transfer_card(cx);
        }
        let sheet = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .rounded(px(15.))
            .bg(rgb(SEND_SHEET))
            .text_color(rgb(SEND_INK))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(10.));
        if self.model.files.is_empty() {
            let choosing = self.picker_open || self.model.validating;
            return sheet
                .justify_center()
                .child(
                    div()
                        .text_center()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(text.share_tagline),
                )
                .child(
                    div()
                        .text_center()
                        .text_size(px(11.))
                        .text_color(rgb(SEND_MUTED))
                        .child(text.share_hint),
                )
                .child(
                    button(
                        "compact-choose-files",
                        if self.picker_open {
                            text.opening_picker
                        } else if self.model.validating {
                            text.footer_reading
                        } else {
                            text.choose_files
                        },
                        true,
                    )
                    .when(choosing, |button| button.opacity(0.5))
                    .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                )
                .children(
                    self.model
                        .error
                        .as_deref()
                        .map(|error| self.compact_send_error(error)),
                )
                .into_any_element();
        }
        sheet
            .child(self.compact_file_summary(cx))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap(px(4.))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(text.send_to_nearby),
                    )
                    .child(
                        div()
                            .id("compact-refresh")
                            .flex_shrink_0()
                            .py(px(3.))
                            .px(px(4.))
                            .text_size(px(10.))
                            .text_color(rgb(SEND_ACCENT))
                            .when(!self.model.scanning, |item| item.cursor_pointer())
                            .child(if self.model.scanning {
                                text.searching
                            } else {
                                text.search_again
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
                    ),
            )
            .child(self.compact_device_grid(cx))
            .children(
                self.model
                    .error
                    .as_deref()
                    .map(|error| self.compact_send_error(error)),
            )
            .into_any_element()
    }

    fn compact_file_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let can_open = self.can_open_files();
        compact_send_card()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .child(self.file_previews())
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(SEND_MUTED))
                            .child(file_size(self.model.bytes)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .text_size(px(10.))
                            .child(
                                div()
                                    .id("compact-open-file")
                                    .text_color(rgb(if can_open {
                                        SEND_ACCENT
                                    } else {
                                        SEND_MUTED
                                    }))
                                    .when(can_open, |button| button.cursor_pointer())
                                    .child(if self.opening_files.is_some() {
                                        text.open_in_progress
                                    } else if self.model.files.len() > 1 {
                                        text.open_all
                                    } else {
                                        text.open_file
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.open_files(cx))),
                            )
                            .child(
                                div()
                                    .id("compact-replace-files")
                                    .cursor_pointer()
                                    .text_color(rgb(SEND_ACCENT))
                                    .child(if self.picker_open {
                                        text.selecting
                                    } else if self.model.validating {
                                        text.reading_short
                                    } else {
                                        text.replace_files
                                    })
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.choose(window, cx)),
                                    ),
                            ),
                    ),
            )
    }

    fn compact_device_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let device_grid = div().flex_1().min_h_0().flex().flex_col();
        if self.model.peers.is_empty() {
            let disconnected = self.model.status.is_some() && !self.model.connection_ready();
            return device_grid
                .justify_center()
                .gap(px(5.))
                .items_center()
                .text_center()
                .text_color(rgb(SEND_MUTED))
                .child(div().text_size(px(12.)).child(if disconnected {
                    text.connection_not_ready
                } else if self.model.scanning {
                    text.scanning
                } else {
                    text.no_receivers
                }))
                .child(div().text_size(px(10.)).child(if disconnected {
                    text.reconnect_search
                } else {
                    text.device_awake_hint
                }))
                .into_any_element();
        }
        let can_send = self.model.connection_ready()
            && !self.model.validating
            && !self.picker_open
            && !self.firmware.busy
            && self.queued_at.is_none();
        device_grid
            .id("compact-nearby-devices")
            .visible_scrollbar()
            .gap(px(7.))
            .children(self.model.peers.chunks(2).map(|pair| {
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(7.))
                    .children(pair.iter().map(|peer| {
                        let instance = peer.service.instance.clone();
                        compact_send_card()
                            .id(SharedString::from(format!("compact-peer-{instance}")))
                            .flex_1()
                            .min_w_0()
                            .flex_col()
                            .gap(px(5.))
                            .when(can_send, |card| {
                                card.cursor_pointer().hover(|card| {
                                    card.bg(rgb(0xf6faff)).border_color(rgb(0x80a7d4))
                                })
                            })
                            .when(!can_send, |card| card.opacity(0.6))
                            .child(compact_device_glyph())
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(peer.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(SEND_ACCENT))
                                    .child(text.send),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_send {
                                    this.model.selected = Some(instance.clone());
                                    this.send(cx);
                                    cx.notify();
                                }
                            }))
                    }))
                    .when(pair.len() == 1, |row| row.child(div().flex_1().min_w_0()))
            }))
            .into_any_element()
    }

    fn compact_transfer_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let complete = self.model.phase == Phase::Complete;
        let cancelling = self.model.phase == Phase::Cancelling;
        let busy = self.model.phase.busy();
        let action = if cancelling {
            text.cancelling
        } else if busy {
            text.cancel_send
        } else if complete {
            text.done
        } else {
            text.back_action
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .id("compact-transfer-scroll")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .visible_scrollbar()
                    .child(self.send_readout()),
            )
            .children(
                self.model
                    .error
                    .as_deref()
                    .map(|error| self.compact_send_error(error)),
            )
            .child(
                div().flex().flex_shrink_0().justify_end().child(
                    div()
                        .id("compact-transfer-action")
                        .rounded(px(10.))
                        .bg(rgb(0xe6eef9))
                        .px(px(17.))
                        .py(px(8.))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(SEND_ACCENT))
                        .when(!cancelling, |action| {
                            action
                                .cursor_pointer()
                                .hover(|action| action.bg(rgb(0xd9e8fa)))
                        })
                        .child(action)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.model.phase == Phase::Cancelling {
                                return;
                            }
                            if this.model.phase.busy() {
                                this.cancel(cx);
                            } else if this.model.phase == Phase::Complete {
                                this.reset(window, cx);
                            } else {
                                this.model.phase = Phase::Ready;
                                this.model.error = None;
                                cx.notify();
                            }
                        })),
                ),
            )
            .into_any_element()
    }

    pub(super) fn compact_send_error(&self, error: &str) -> AnyElement {
        div()
            .id("compact-send-error")
            .flex_shrink_0()
            .max_h(px(52.))
            .visible_scrollbar()
            .text_size(px(10.))
            .text_color(rgb(0x97451d))
            .whitespace_normal()
            .child(error.to_owned())
            .into_any_element()
    }
}

fn compact_send_card() -> Div {
    div()
        .flex()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(0xd7e2ee))
        .bg(rgb(0xffffff))
        .p(px(8.))
}

fn compact_device_glyph() -> impl IntoElement {
    div()
        .size(px(30.))
        .rounded(px(8.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(0xe5effa))
        .child(
            div()
                .w(px(10.))
                .h(px(16.))
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(SEND_ACCENT))
                .flex()
                .items_end()
                .justify_center()
                .pb(px(2.))
                .child(
                    div()
                        .w(px(3.))
                        .h(px(1.))
                        .rounded_full()
                        .bg(rgb(SEND_ACCENT)),
                ),
        )
}
