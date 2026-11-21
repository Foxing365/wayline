use super::{Capsule, Phase, file_size};
use crate::receiving::Phase as ReceivePhase;
use crate::scroll_area::ScrollbarExt;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Mark {
    Send,
    Receive,
    Done,
    Stopped,
    Error,
}

struct Readout {
    mark: Mark,
    title: &'static str,
    peer: String,
    files: Option<String>,
    bytes: Option<u64>,
    total: Option<u64>,
    file_bytes: Option<u64>,
    speed: Option<u64>,
    elapsed: Option<Duration>,
    network: String,
    note: Option<String>,
    active: bool,
}

fn percentage(bytes: u64, total: u64) -> Option<u8> {
    (total > 0).then(|| ((u128::from(bytes.min(total)) * 100) / u128::from(total)) as u8)
}

fn rate_value(value: f64) -> String {
    if value == 0. {
        "0".into()
    } else if value < 0.01 {
        "<0.01".into()
    } else {
        format!("{value:.2}")
    }
}

fn rate_units(bytes_per_second: u64) -> (String, String) {
    let bytes = bytes_per_second as f64;
    (
        format!("{} MiB", rate_value(bytes / 1_048_576.)),
        format!("{} Mbps", rate_value(bytes * 8. / 1_000_000.)),
    )
}

impl Capsule {
    pub(super) fn send_readout(&self) -> AnyElement {
        let text = self.language.strings();
        let model = &self.model;
        let (bytes, total) = match model.phase {
            Phase::Uploading { sent, total } => (Some(sent), Some(total)),
            // The sender emits Confirming only after checking the full archive was written.
            Phase::Confirming | Phase::Complete if model.send_transferred_bytes > 0 => (
                Some(model.send_transferred_bytes),
                Some(model.send_transferred_bytes),
            ),
            _ => (
                (model.send_transferred_bytes > 0).then_some(model.send_transferred_bytes),
                None,
            ),
        };
        let mark = match model.phase {
            Phase::Complete => Mark::Done,
            Phase::Cancelled | Phase::Declined | Phase::Cancelling => Mark::Stopped,
            Phase::Failed => Mark::Error,
            _ => Mark::Send,
        };
        let note = match model.phase {
            Phase::Preparing => Some(text.transfer_preparing),
            Phase::Waiting => Some(text.transfer_waiting),
            Phase::Confirming => Some(text.transfer_confirming),
            Phase::Complete => Some(text.transfer_complete),
            Phase::Cancelled => Some(text.transfer_cancelled),
            Phase::Declined => Some(text.transfer_declined),
            Phase::Cancelling => Some(text.transfer_cancelling),
            Phase::Failed => Some(text.transfer_failed),
            Phase::Uploading { sent: 0, .. } => Some(text.transfer_keep_near),
            _ => None,
        };
        let (channel, format) = model.transfer_metadata();
        self.transfer_readout(
            Readout {
                mark,
                title: model.phase.title(self.language),
                peer: model.target.clone(),
                files: match model.files.as_slice() {
                    [] => None,
                    [path] => path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                    files => Some(self.language.file_count(files.len())),
                },
                bytes,
                total,
                file_bytes: Some(model.send_file_bytes),
                speed: model.speed_bytes_per_second,
                elapsed: model.send_elapsed(Instant::now()),
                network: self.language.transfer_network(channel, format),
                note: note.map(str::to_owned),
                active: model.phase.busy(),
            },
            false,
        )
    }

    pub(super) fn receiving_view(&self, mini: bool) -> AnyElement {
        let text = self.language.strings();
        let transfer = self.receive.active.as_ref().unwrap();
        let (title, bytes, total, note) = match transfer.phase {
            ReceivePhase::Waiting => (
                text.receive_waiting_title,
                None,
                None,
                Some(text.receive_waiting_detail),
            ),
            ReceivePhase::Receiving { received, total } => {
                (text.receiving_title, Some(received), total, None)
            }
            ReceivePhase::Confirming => (
                text.receive_confirming_title,
                Some(transfer.transferred_bytes),
                // UploadComplete ends the byte stream before the receiver publishes the files.
                Some(transfer.transferred_bytes),
                Some(text.receive_confirming_detail),
            ),
        };
        let (channel, format) = self.receive.transfer_metadata();
        let card = self.transfer_readout(
            Readout {
                mark: Mark::Receive,
                title,
                peer: self
                    .language
                    .transfer_files(&transfer.sender, transfer.files),
                files: match transfer.file_names.as_slice() {
                    [name] if !mini => Some(name.clone()),
                    _ => None,
                },
                bytes,
                total,
                file_bytes: transfer.file_bytes,
                speed: self.receive.speed_bytes_per_second,
                elapsed: self.receive.receive_elapsed(Instant::now()),
                network: self.language.transfer_network(channel, format),
                note: note.map(str::to_owned),
                active: true,
            },
            mini,
        );
        if mini {
            return card;
        }
        div()
            .id("receive-readout-scroll")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .visible_scrollbar()
            .child(
                div()
                    .w_full()
                    .flex_grow()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(9.))
                    .child(card)
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.compact_send_view {
                                0x526278
                            } else {
                                0xa5b1c0
                            }))
                            .whitespace_normal()
                            .child(
                                self.language
                                    .saved_to(&transfer.save_dir.display().to_string()),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn transfer_readout(&self, data: Readout, mini: bool) -> AnyElement {
        let light = self.compact_send_view;
        let (surface, border, ink, muted, track) = if light {
            (0xffffff, 0xd0dfef, 0x17293c, 0x475d73, 0xdce6f1)
        } else {
            (0x20252d, 0x36404d, 0xf2f6fc, 0xa5b1c0, 0x394452)
        };
        let accent = match (data.mark, light) {
            (Mark::Done, true) => 0x157355,
            (Mark::Done, false) => 0x6fdbb1,
            (Mark::Error | Mark::Stopped, true) => 0x985222,
            (Mark::Error | Mark::Stopped, false) => 0xffb87f,
            (Mark::Receive, true) => 0x007775,
            (Mark::Receive, false) => 0x65ded1,
            (_, true) => 0x225ca0,
            (_, false) => 0x83baff,
        };
        let total = data.total.filter(|&value| value > 0);
        let progress = TransferProgress::new(data.bytes, total, data.mark, data.active);
        let text = self.language.strings();
        let animate = self.animate_send_view && data.active && !matches!(data.mark, Mark::Stopped);
        let padding = if mini { 9. } else { 17. };
        let heading = div()
            .flex()
            .items_center()
            .gap(px(if mini { 7. } else { 10. }))
            .child(status_glyph(data.mark, light, accent, mini, animate))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(if mini { 13. } else { 19. }))
                            .line_height(px(if mini { 17. } else { 24. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(data.title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if mini { 10. } else { 12. }))
                            .text_color(rgb(muted))
                            .child(data.peer),
                    ),
            )
            .children(progress.percent().map(|percent| {
                div()
                    .flex_shrink_0()
                    .text_size(px(if mini { 17. } else { 28. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(accent))
                    .child(format!("{percent}%"))
            }));
        let header = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(if mini { 6. } else { 15. }))
            .p(px(padding))
            .bg(Hsla::from(rgb(accent)).opacity(if light { 0.07 } else { 0.09 }))
            .child(heading)
            .when(!matches!(progress, TransferProgress::Idle), |header| {
                header.child(progress_rail(progress, accent, track, mini, animate))
            });

        let transferred = data.bytes.map(|bytes| match total {
            Some(total) => format!("{} / {}", file_size(bytes), file_size(total)),
            None => file_size(bytes),
        });
        let source_size = data.file_bytes.filter(|size| Some(*size) != total);
        let rate = data.speed.map(rate_units);
        let mut card = div()
            .w_full()
            .flex_shrink_0()
            .when(!mini, |card| card.flex_grow())
            .flex()
            .flex_col()
            .rounded(px(if mini { 11. } else { 17. }))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(surface))
            .text_color(rgb(ink))
            .child(header);

        if mini {
            let compact_amount = match (transferred, total, data.mark) {
                (Some(amount), None, Mark::Receive) => Some(self.language.received_size(&amount)),
                (Some(amount), _, _) => Some(amount),
                _ => source_size
                    .map(|bytes| format!("{}  {}", text.transfer_source_size, file_size(bytes))),
            };
            if compact_amount.is_some() || rate.is_some() {
                card = card.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(6.))
                        .px(px(padding))
                        .py(px(5.))
                        .text_size(px(11.))
                        .children(compact_amount.map(|amount| div().child(amount)))
                        .children(rate.map(|(mib, mbps)| {
                            div()
                                .text_color(rgb(accent))
                                .child(format!("{} · {mbps}", self.language.transfer_speed(&mib)))
                        })),
                );
            }
            return card.into_any_element();
        }

        let mut body = div()
            .flex_grow()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(11.))
            .p(px(padding));
        if let Some(amount) = transferred {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(muted))
                            .child(text.transfer_stream_bytes),
                    )
                    .child(
                        div()
                            .text_size(px(21.))
                            .line_height(px(26.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(amount),
                    ),
            );
        }
        if let Some((mib, mbps)) = rate {
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(accent))
                            .child(self.language.transfer_speed(&mib)),
                    )
                    .child(div().text_size(px(11.)).text_color(rgb(muted)).child(mbps)),
            );
        }
        let has_body = data.bytes.is_some() || data.speed.is_some() || data.note.is_some();
        if let Some(note) = data.note {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .whitespace_normal()
                    .text_color(rgb(muted))
                    .child(note),
            );
        }
        if has_body {
            card = card.child(body);
        }

        let network = match data.elapsed {
            Some(duration) => format!(
                "{}  ·  {}",
                data.network,
                self.language
                    .transfer_elapsed(duration.as_millis().min(u64::MAX as u128) as u64)
            ),
            None => data.network,
        };
        let mut footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_t_1()
            .border_color(rgb(border))
            .px(px(padding))
            .py(px(10.));
        if data.files.is_some() || source_size.is_some() {
            footer = footer.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .children(
                        data.files
                            .map(|name| div().flex_1().min_w_0().truncate().child(name)),
                    )
                    .children(source_size.map(|bytes| {
                        div().flex_shrink_0().text_color(rgb(muted)).child(format!(
                            "{}  {}",
                            text.transfer_source_size,
                            file_size(bytes)
                        ))
                    })),
            );
        }
        card.child(
            footer.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(muted))
                    .whitespace_normal()
                    .child(network),
            ),
        )
        .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TransferProgress {
    Bytes(u64, u64),
    Complete,
    Indeterminate,
    Idle,
}

impl TransferProgress {
    fn new(bytes: Option<u64>, total: Option<u64>, mark: Mark, active: bool) -> Self {
        if matches!(mark, Mark::Done) {
            Self::Complete
        } else if let Some((bytes, total)) = bytes.zip(total.filter(|&size| size > 0)) {
            Self::Bytes(bytes.min(total), total)
        } else if active && !matches!(mark, Mark::Stopped | Mark::Error) {
            Self::Indeterminate
        } else {
            Self::Idle
        }
    }

    fn percent(self) -> Option<u8> {
        match self {
            Self::Bytes(bytes, total) => percentage(bytes, total),
            Self::Complete => Some(100),
            _ => None,
        }
    }
}

fn progress_rail(
    progress: TransferProgress,
    accent: u32,
    track: u32,
    mini: bool,
    animate: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let radius = bounds.size.height / 2.;
            let length = bounds.size.width;
            let ink = Hsla::from(rgb(accent));
            let mut paint = |start: f32, end: f32, color: Hsla| {
                if end > start {
                    let origin = bounds.origin + point(length * start, px(0.));
                    let width = length * (end - start);
                    window.paint_quad(
                        fill(Bounds::new(origin, size(width, bounds.size.height)), color)
                            .corner_radii(radius.min(width / 2.)),
                    );
                }
            };
            paint(0., 1., rgb(track).into());
            match progress {
                TransferProgress::Bytes(bytes, total) => {
                    paint(0., (bytes as f64 / total as f64) as f32, ink)
                }
                TransferProgress::Complete => paint(0., 1., ink),
                TransferProgress::Indeterminate if animate => {
                    let start =
                        (0.5 - 0.5 * (activity_phase() * std::f32::consts::TAU).cos()) * 0.70;
                    paint(start, start + 0.30, ink);
                    window.request_animation_frame();
                }
                // A patterned rail conveys unknown length even with Windows animations disabled.
                TransferProgress::Indeterminate => {
                    for segment in 0..8 {
                        let start = segment as f32 / 8.;
                        paint(start + 0.014, start + 0.111, ink.opacity(0.60));
                    }
                }
                TransferProgress::Idle => {}
            }
        },
    )
    .w_full()
    .h(px(if mini { 5. } else { 10. }))
    .flex_shrink_0()
}

fn activity_phase() -> f32 {
    const PERIOD_SECONDS: f64 = 2.2;
    static CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    (CLOCK.get_or_init(Instant::now).elapsed().as_secs_f64() % PERIOD_SECONDS / PERIOD_SECONDS)
        as f32
}

// The moving ring indicates activity. Progress and rate use protocol counters.
fn status_glyph(
    mark: Mark,
    light: bool,
    accent: u32,
    mini: bool,
    animate: bool,
) -> impl IntoElement {
    let side = if mini { 32. } else { 44. };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let unit = side / 44.;
            let at = |x: f32, y: f32| center + point(px(x * unit), px(y * unit));
            let tint = Hsla::from(rgb(accent));
            let disc = |window: &mut Window, radius: f32, color: Hsla| {
                let radius = px(radius * unit);
                window.paint_quad(
                    fill(
                        Bounds::new(
                            center - point(radius, radius),
                            size(radius * 2., radius * 2.),
                        ),
                        color,
                    )
                    .corner_radii(radius),
                );
            };
            let phase = if animate { activity_phase() } else { 0.35 };
            disc(window, 16. + 5. * phase, tint.opacity(0.14 * (1. - phase)));
            disc(window, 15., tint.opacity(if light { 0.12 } else { 0.18 }));
            if matches!(mark, Mark::Send | Mark::Receive) {
                let direction = if matches!(mark, Mark::Receive) {
                    -1.
                } else {
                    1.
                };
                let angle = phase * std::f32::consts::TAU * direction;
                let bead = px(2.1 * unit);
                let position = at(17.5 * angle.cos(), 17.5 * angle.sin());
                window.paint_quad(
                    fill(
                        Bounds::new(position - point(bead, bead), size(bead * 2., bead * 2.)),
                        tint,
                    )
                    .corner_radii(bead),
                );
            }
            let mut glyph = PathBuilder::stroke(px(1.7 * unit));
            let mut stroke = |points: &[(f32, f32)]| {
                for (index, &(x, y)) in points.iter().enumerate() {
                    if index == 0 {
                        glyph.move_to(at(x, y));
                    } else {
                        glyph.line_to(at(x, y));
                    }
                }
            };
            if light {
                // The compact theme's cat face frames the live direction or outcome.
                stroke(&[
                    (-10., 2.),
                    (-10., -9.),
                    (-4., -5.),
                    (4., -5.),
                    (10., -9.),
                    (10., 2.),
                    (8., 7.),
                    (4., 10.),
                    (-4., 10.),
                    (-8., 7.),
                    (-10., 2.),
                ]);
            }
            let y = if light { 2. } else { 0. };
            match mark {
                Mark::Send | Mark::Receive => {
                    let direction = if matches!(mark, Mark::Send) { -1. } else { 1. };
                    stroke(&[(0., y - 4.5 * direction), (0., y + 4.5 * direction)]);
                    stroke(&[
                        (-4., y + 0.5 * direction),
                        (0., y + 4.5 * direction),
                        (4., y + 0.5 * direction),
                    ]);
                    if !light {
                        stroke(&[
                            (-7., -4. * direction),
                            (-7., -8. * direction),
                            (7., -8. * direction),
                            (7., -4. * direction),
                        ]);
                    }
                }
                Mark::Done => stroke(&[(-4.5, y), (-1., y + 3.5), (5., y - 3.)]),
                Mark::Stopped => {
                    stroke(&[(-3., y - 3.), (3., y + 3.)]);
                    stroke(&[(3., y - 3.), (-3., y + 3.)]);
                }
                Mark::Error => {
                    stroke(&[(0., y - 4.5), (0., y + 1.5)]);
                    stroke(&[(0., y + 3.5), (0., y + 5.)]);
                }
            }
            if let Ok(path) = glyph.build() {
                window.paint_path(path, tint);
            }
            if animate {
                window.request_animation_frame();
            }
        },
    )
    .size(px(side))
    .flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::{Mark, TransferProgress, percentage, rate_units};

    #[test]
    fn active_send_and_receive_keep_a_rail_without_an_advertised_total() {
        for mark in [Mark::Send, Mark::Receive] {
            for (bytes, total) in [(None, None), (Some(120), None), (Some(120), Some(0))] {
                let progress = TransferProgress::new(bytes, total, mark, true);
                assert_eq!(progress, TransferProgress::Indeterminate);
                assert_eq!(progress.percent(), None);
            }
            let starting = TransferProgress::new(Some(0), Some(100), mark, true);
            assert_eq!(starting.percent(), Some(0));
            let moving = TransferProgress::new(Some(320), Some(1_000), mark, true);
            assert_eq!(moving.percent(), Some(32));
        }
    }

    #[test]
    fn completion_fills_the_rail_and_unknown_stopped_work_has_no_activity() {
        let done = TransferProgress::new(None, None, Mark::Done, false);
        assert_eq!(done.percent(), Some(100));
        for mark in [Mark::Stopped, Mark::Error] {
            for active in [false, true] {
                assert_eq!(
                    TransferProgress::new(Some(120), None, mark, active),
                    TransferProgress::Idle
                );
            }
        }
    }
    #[test]
    fn both_rate_units_use_the_same_sample_including_stalls() {
        assert_eq!(
            rate_units(1_048_576),
            ("1.00 MiB".into(), "8.39 Mbps".into())
        );
        assert_eq!(rate_units(0), ("0 MiB".into(), "0 Mbps".into()));
        assert_eq!(rate_units(1), ("<0.01 MiB".into(), "<0.01 Mbps".into()));
    }
    #[test]
    fn byte_percentage_is_bounded_and_only_reaches_100_at_the_total() {
        assert_eq!(percentage(0, 0), None);
        assert_eq!(percentage(999, 1000), Some(99));
        assert_eq!(percentage(u64::MAX - 1, u64::MAX), Some(99));
        assert_eq!(percentage(u64::MAX, u64::MAX - 1), Some(100));
    }
}
