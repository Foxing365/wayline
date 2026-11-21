use super::{Capsule, Phase, file_size};
use crate::receiving::Phase as ReceivePhase;
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
                None,
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
            .overflow_y_scroll()
            .flex()
            .flex_col()
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
        let total = data.total.filter(|total| *total > 0);
        let counts = data.bytes.zip(total);
        let percent = counts
            .and_then(|(bytes, total)| percentage(bytes, total))
            .or_else(|| matches!(data.mark, Mark::Done).then_some(100));
        let fraction = counts
            .map(|(bytes, total)| bytes.min(total) as f64 / total as f64)
            .or_else(|| matches!(data.mark, Mark::Done).then_some(1.0));
        let text = self.language.strings();
        let amount = data.bytes.map(|bytes| match total {
            Some(total) => format!(
                "{}  {} / {}",
                text.transfer_stream_bytes,
                file_size(bytes),
                file_size(total)
            ),
            None if matches!(data.mark, Mark::Receive) => {
                self.language.received_size(&file_size(bytes))
            }
            None => format!("{}  {}", text.transfer_stream_bytes, file_size(bytes)),
        });
        let source_size = data.file_bytes.filter(|size| Some(*size) != total);
        let animate = self.animate_send_view && data.active && !matches!(data.mark, Mark::Stopped);
        let mut card = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(if mini { 5. } else { 10. }))
            .p(px(if mini { 8. } else { 13. }))
            .rounded(px(if light { 10. } else { 15. }))
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(surface))
            .text_color(rgb(ink))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
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
                                    .text_size(px(if mini { 12. } else { 16. }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(data.title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(rgb(muted))
                                    .child(data.peer),
                            ),
                    )
                    .children(percent.map(|value| {
                        div()
                            .flex_shrink_0()
                            .text_size(px(if mini { 14. } else { 22. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(accent))
                            .child(format!("{value}%"))
                    })),
            );
        if let Some(fraction) = fraction {
            card = card.child(
                div()
                    .w_full()
                    .h(px(if mini { 3. } else { 5. }))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(track))
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction as f32))
                            .rounded_full()
                            .bg(rgb(accent)),
                    ),
            );
        }
        if amount.is_some() || source_size.is_some() {
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .children(amount.map(|amount| div().child(amount)))
                    .children(source_size.map(|size| {
                        div().text_color(rgb(muted)).child(format!(
                            "{}  {}",
                            text.transfer_source_size,
                            file_size(size)
                        ))
                    })),
            );
        }
        if let Some(bytes) = data.speed {
            let (mib, mbps) = rate_units(bytes);
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(accent))
                            .child(self.language.transfer_speed(&mib)),
                    )
                    .child(div().text_size(px(10.)).text_color(rgb(muted)).child(mbps)),
            );
        }
        if !mini {
            let footer = match data.elapsed {
                Some(duration) => format!(
                    "{}  ·  {}",
                    data.network,
                    self.language
                        .transfer_elapsed(duration.as_millis().min(u64::MAX as u128) as u64)
                ),
                None => data.network,
            };
            card = card
                .children(data.note.map(|note| {
                    div()
                        .text_size(px(11.))
                        .whitespace_normal()
                        .text_color(rgb(muted))
                        .child(note)
                }))
                .child(
                    div()
                        .border_t_1()
                        .border_color(rgb(border))
                        .pt(px(6.))
                        .text_size(px(10.))
                        .whitespace_normal()
                        .text_color(rgb(muted))
                        .child(footer),
                );
        }
        card.into_any_element()
    }
}

// The moving ring indicates activity. Progress and rate use protocol counters.
fn status_glyph(
    mark: Mark,
    light: bool,
    accent: u32,
    mini: bool,
    animate: bool,
) -> impl IntoElement {
    const PERIOD: f32 = 2.2;
    static CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
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
            let phase = if animate {
                CLOCK.get_or_init(Instant::now).elapsed().as_secs_f32() % PERIOD / PERIOD
            } else {
                0.35
            };
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
    use super::{percentage, rate_units};
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
