use super::{Capsule, MUTED, Phase, SURFACE, button, file_size};
use crate::scroll_area::ScrollbarExt;
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SendLayout {
    #[default]
    Classic,
    NekoLight,
    NekoDark,
}

impl SendLayout {
    fn label(self, text: &crate::i18n::Strings) -> &'static str {
        match self {
            Self::Classic => text.send_layout_classic,
            Self::NekoLight => text.send_layout_compact,
            Self::NekoDark => text.send_layout_compact_dark,
        }
    }
}

#[derive(Clone, Copy)]
struct SendPalette {
    sheet: u32,
    surface: u32,
    border: u32,
    ink: u32,
    muted: u32,
    accent: u32,
    hover: u32,
    hover_border: u32,
    tint: u32,
    tint_hover: u32,
    error: u32,
}

impl SendPalette {
    fn for_layout(layout: SendLayout) -> Self {
        if layout == SendLayout::NekoLight {
            Self {
                sheet: 0xeef2f7,
                surface: 0xffffff,
                border: 0xd7e2ee,
                ink: 0x172538,
                muted: 0x4d5d74,
                accent: 0x205da9,
                hover: 0xf6faff,
                hover_border: 0x80a7d4,
                tint: 0xe5effa,
                tint_hover: 0xd9e8fa,
                error: 0x97451d,
            }
        } else {
            Self {
                sheet: 0x1c222b,
                surface: 0x242c37,
                border: 0x384657,
                ink: 0xeff4fc,
                muted: 0xafbbcd,
                accent: 0x8abfff,
                hover: 0x2c3b4e,
                hover_border: 0x6489b1,
                tint: 0x293b52,
                tint_hover: 0x334c69,
                error: 0xffba93,
            }
        }
    }
}

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
                div().flex().flex_col().gap(px(6.)).children(
                    [
                        ("classic-send-layout", SendLayout::Classic),
                        ("compact-send-layout", SendLayout::NekoLight),
                        ("dark-compact-send-layout", SendLayout::NekoDark),
                    ]
                    .map(|(id, layout)| {
                        button(id, layout.label(&text), self.send_layout == layout)
                            .h(px(32.))
                            .w_full()
                            .rounded(px(8.))
                            .text_size(px(11.))
                            .when(self.send_layout == layout, |choice| {
                                choice.text_color(rgb(0x101a27))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_send_layout(layout, cx);
                            }))
                    }),
                ),
            )
            .into_any_element()
    }

    pub(super) fn send_layout_refresh_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let layout = self
            .pending_send_layout
            .unwrap_or(self.send_layout)
            .label(&text);
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
        let color = SendPalette::for_layout(self.send_layout);
        if self.model.phase != Phase::Ready {
            return self.compact_transfer_card(cx);
        }
        let sheet = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .rounded(px(15.))
            .bg(rgb(color.sheet))
            .text_color(rgb(color.ink))
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
                        .text_color(rgb(color.muted))
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
                    .bg(rgb(color.accent))
                    .text_color(rgb(if self.send_layout == SendLayout::NekoLight {
                        0xffffff
                    } else {
                        0x101a27
                    }))
                    .hover(move |button| button.bg(rgb(color.accent)).opacity(0.9))
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
            .child(self.compact_file_summary(color, cx))
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
                            .text_color(rgb(color.accent))
                            .when(!self.model.scanning, |item| item.cursor_pointer())
                            .child(if self.model.scanning {
                                text.searching
                            } else {
                                text.search_again
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
                    ),
            )
            .child(self.compact_device_list(color, cx))
            .children(
                self.model
                    .error
                    .as_deref()
                    .map(|error| self.compact_send_error(error)),
            )
            .into_any_element()
    }

    fn compact_file_summary(&self, color: SendPalette, cx: &mut Context<Self>) -> impl IntoElement {
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
        compact_send_card(color)
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
                            .text_color(rgb(color.muted))
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
                                        color.accent
                                    } else {
                                        color.muted
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
                                    .text_color(rgb(color.accent))
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

    fn compact_device_list(&self, color: SendPalette, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let devices = div().flex_1().min_h_0().flex().flex_col();
        if self.model.peers.is_empty() {
            let disconnected = self.model.status.is_some() && !self.model.connection_ready();
            return devices
                .justify_center()
                .gap(px(5.))
                .items_center()
                .text_center()
                .text_color(rgb(color.muted))
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
        devices
            .id("compact-nearby-devices")
            .visible_scrollbar()
            .gap(px(7.))
            .children(self.model.peers.iter().map(|peer| {
                let instance = peer.service.instance.clone();
                compact_send_card(color)
                    .id(SharedString::from(format!("compact-peer-{instance}")))
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(9.))
                    .when(can_send, |card| {
                        card.cursor_pointer().hover(move |card| {
                            card.bg(rgb(color.hover))
                                .border_color(rgb(color.hover_border))
                        })
                    })
                    .when(!can_send, |card| card.opacity(0.6))
                    .child(compact_device_glyph(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(peer.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(color.accent))
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
            .into_any_element()
    }

    fn compact_transfer_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.language.strings();
        let color = SendPalette::for_layout(self.send_layout);
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
                        .bg(rgb(color.tint))
                        .px(px(17.))
                        .py(px(8.))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(color.accent))
                        .when(!cancelling, |action| {
                            action
                                .cursor_pointer()
                                .hover(move |action| action.bg(rgb(color.tint_hover)))
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
        let color = SendPalette::for_layout(self.send_layout);
        div()
            .id("compact-send-error")
            .flex_shrink_0()
            .max_h(px(52.))
            .visible_scrollbar()
            .text_size(px(10.))
            .text_color(rgb(color.error))
            .whitespace_normal()
            .child(error.to_owned())
            .into_any_element()
    }
}

fn compact_send_card(color: SendPalette) -> Div {
    div()
        .flex()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(color.border))
        .bg(rgb(color.surface))
        .p(px(8.))
}

fn compact_device_glyph(color: SendPalette) -> impl IntoElement {
    div()
        .size(px(30.))
        .rounded(px(8.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(color.tint))
        .child(
            div()
                .w(px(10.))
                .h(px(16.))
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(color.accent))
                .flex()
                .items_end()
                .justify_center()
                .pb(px(2.))
                .child(
                    div()
                        .w(px(3.))
                        .h(px(1.))
                        .rounded_full()
                        .bg(rgb(color.accent)),
                ),
        )
}
