//! Native scrolling with a persistent, proportional scrollbar when content overflows.
use gpui::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

const GUTTER: f32 = 12.;
const INSET: f32 = 2.;
const MIN_THUMB: f32 = 24.;
const WHEEL_HOLD: Duration = Duration::from_millis(90);
const WHEEL_FADE: Duration = Duration::from_millis(210);
const WHEEL_FEEDBACK: Duration = WHEEL_HOLD.saturating_add(WHEEL_FADE);

fn wheel_moved(before: Pixels, after: Pixels, max: Pixels) -> bool {
    before.clamp(-max, px(0.)) != after.clamp(-max, px(0.))
}

#[derive(Clone, Default)]
pub struct ScrollState {
    handle: ScrollHandle,
    grab_offset: Rc<Cell<Option<f32>>>,
    wheel_feedback: Rc<Cell<Option<WheelFeedback>>>,
    feedback_expiry: Rc<RefCell<Option<Task<()>>>>,
}

#[derive(Clone, Copy)]
struct WheelFeedback {
    started: Instant,
    animated: bool,
}

impl WheelFeedback {
    fn intensity(self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= WHEEL_FEEDBACK {
            return 0.;
        }
        if !self.animated {
            return 1.;
        }
        1. - (elapsed.saturating_sub(WHEEL_HOLD).as_secs_f32() / WHEEL_FADE.as_secs_f32())
    }
}

pub(crate) fn client_area_animation_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    let mut enabled = 0_i32;
    // SPI_GETCLIENTAREAANIMATION writes one Win32 BOOL to the supplied pointer.
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && enabled != 0
}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    track_top: f32,
    track_height: f32,
    thumb_top: f32,
    thumb_height: f32,
    max_scroll: f32,
}

impl Geometry {
    fn new(top: f32, viewport: f32, max_scroll: f32, offset: f32) -> Option<Self> {
        let track_height = viewport - INSET * 2.;
        if max_scroll <= 0.5 || track_height <= 0. {
            return None;
        }
        let min_thumb = MIN_THUMB.min(track_height * 0.8);
        let thumb_height = (track_height * viewport / (viewport + max_scroll))
            .max(min_thumb)
            .min(track_height);
        let track_top = top + INSET;
        let thumb_top =
            track_top + (-offset / max_scroll).clamp(0., 1.) * (track_height - thumb_height);
        Some(Self {
            track_top,
            track_height,
            thumb_top,
            thumb_height,
            max_scroll,
        })
    }

    fn scroll_at(self, pointer: f32, grab_offset: f32) -> f32 {
        let travel = self.track_height - self.thumb_height;
        if travel <= 0. {
            return 0.;
        }
        ((pointer - self.track_top - grab_offset) / travel).clamp(0., 1.) * self.max_scroll
    }
}

/// Keeps the original div's sizing, child layout, wheel handling and element identity.
pub struct ScrollArea {
    inner: Option<Stateful<Div>>,
}

pub trait ScrollbarExt {
    fn visible_scrollbar(self) -> ScrollArea;
}

impl ScrollbarExt for Stateful<Div> {
    fn visible_scrollbar(self) -> ScrollArea {
        ScrollArea {
            inner: Some(self.overflow_y_scroll().scrollbar_width(px(GUTTER))),
        }
    }
}

impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.as_mut().unwrap().style()
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.as_mut().unwrap().extend(elements);
    }
}

impl InteractiveElement for ScrollArea {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.as_mut().unwrap().interactivity()
    }
}

impl StatefulInteractiveElement for ScrollArea {}

impl IntoElement for ScrollArea {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ScrollArea {
    type RequestLayoutState = (<Stateful<Div> as Element>::RequestLayoutState, ScrollState);
    type PrepaintState = (
        <Stateful<Div> as Element>::PrepaintState,
        Option<(Hitbox, Geometry)>,
    );

    fn id(&self) -> Option<ElementId> {
        Element::id(self.inner.as_ref().unwrap())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.inner.as_ref().unwrap().source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let scroll = window.with_element_state(
            id.expect("scroll areas have an ID"),
            |state: Option<ScrollState>, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            },
        );
        self.inner = Some(self.inner.take().unwrap().track_scroll(&scroll.handle));
        let (layout, state) = self
            .inner
            .as_mut()
            .unwrap()
            .request_layout(id, inspector, window, cx);
        (layout, (state, scroll))
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let inner =
            self.inner
                .as_mut()
                .unwrap()
                .prepaint(id, inspector, bounds, &mut state.0, window, cx);
        // ScrollHandle is updated by the div's prepaint; read it only afterwards.
        let geometry = Geometry::new(
            bounds.top().into(),
            bounds.size.height.into(),
            state.1.handle.max_offset().height.into(),
            state.1.handle.offset().y.into(),
        );
        let bar = geometry.map(|geometry| {
            let track = Bounds::new(
                point(bounds.right() - px(GUTTER), bounds.top()),
                size(px(GUTTER), bounds.size.height),
            );
            (
                // Keep ancestor hover active (notably expanded notifications).
                // Mouse handlers below consume scrollbar clicks before content handlers.
                window.insert_hitbox(track, HitboxBehavior::Normal),
                geometry,
            )
        });
        if bar.is_none() {
            state.1.grab_offset.set(None);
            state.1.wheel_feedback.set(None);
            state.1.feedback_expiry.borrow_mut().take();
        }
        (inner, bar)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Compare this area's offset before and after the native listener. An inner
        // scroller consumes the event while it moves, then lets it reach its parent at
        // the boundary without reimplementing GPUI's line-height-aware wheel delta.
        if let Some(hitbox) = &prepaint.0 {
            let wheel_hitbox = hitbox.clone();
            let scroll = state.1.clone();
            let previous_offset = Rc::new(Cell::new(None));
            window.on_mouse_event(move |_event: &ScrollWheelEvent, phase, window, cx| {
                if !wheel_hitbox.should_handle_scroll(window) {
                    return;
                }
                if phase.capture() {
                    previous_offset.set(Some(scroll.handle.offset().y));
                } else if phase.bubble()
                    && let Some(previous) = previous_offset.take()
                    && wheel_moved(
                        previous,
                        scroll.handle.offset().y,
                        scroll.handle.max_offset().height,
                    )
                {
                    if scroll.grab_offset.get().is_none() {
                        // The wheel keeps GPUI's exact offset. Only the visible
                        // thumb accent changes, including for touchpad input.
                        let animated = scroll
                            .wheel_feedback
                            .get()
                            .map_or_else(client_area_animation_enabled, |feedback| {
                                feedback.animated
                            });
                        scroll.wheel_feedback.set(Some(WheelFeedback {
                            started: Instant::now(),
                            animated,
                        }));
                        scroll.feedback_expiry.replace((!animated).then(|| {
                            let activity = scroll.wheel_feedback.clone();
                            window.spawn(cx, async move |cx| {
                                cx.background_executor().timer(WHEEL_FEEDBACK).await;
                                let _ = cx.update(|window, _| {
                                    activity.set(None);
                                    window.refresh();
                                });
                            })
                        }));
                    }
                    cx.stop_propagation();
                }
            });
        }
        self.inner.as_mut().unwrap().paint(
            id,
            inspector,
            bounds,
            &mut state.0,
            &mut prepaint.0,
            window,
            cx,
        );
        let Some((hitbox, geometry)) = &prepaint.1 else {
            return;
        };
        let geometry = *geometry;
        let hovered = hitbox.is_hovered(window);
        let direct = hovered || state.1.grab_offset.get().is_some();
        let mut intensity = 0.;
        let mut width = if direct { 8. } else { 6. };
        if let Some(feedback) = state.1.wheel_feedback.get() {
            intensity = feedback.intensity(Instant::now());
            if intensity == 0. {
                state.1.wheel_feedback.set(None);
            } else if feedback.animated {
                width += (8. - width) * intensity;
                window.request_animation_frame();
            }
        }
        let thumb = rgb(if direct { 0xb4b4be } else { 0x787882 }).blend(Rgba {
            a: intensity,
            ..rgb(0x73b2ff)
        });
        window.set_cursor_style(CursorStyle::Arrow, hitbox);
        window.paint_quad(
            fill(
                Bounds::new(
                    point(hitbox.left() + px(2.), px(geometry.track_top)),
                    size(px(8.), px(geometry.track_height)),
                ),
                rgb(0x303037),
            )
            .corner_radii(px(4.)),
        );
        window.paint_quad(
            fill(
                Bounds::new(
                    point(
                        hitbox.left() + px((GUTTER - width) / 2.),
                        px(geometry.thumb_top),
                    ),
                    size(px(width), px(geometry.thumb_height)),
                ),
                thumb,
            )
            .corner_radii(px(width / 2.)),
        );

        let down_hitbox = hitbox.clone();
        let down = state.1.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble()
                || event.button != MouseButton::Left
                || !down_hitbox.is_hovered(window)
            {
                return;
            }
            let y: f32 = event.position.y.into();
            let grab = if y >= geometry.thumb_top && y <= geometry.thumb_top + geometry.thumb_height
            {
                y - geometry.thumb_top
            } else {
                geometry.thumb_height / 2.
            };
            down.wheel_feedback.set(None);
            down.feedback_expiry.borrow_mut().take();
            down.grab_offset.set(Some(grab));
            down.handle.set_offset(point(
                down.handle.offset().x,
                px(-geometry.scroll_at(y, grab)),
            ));
            window.refresh();
            cx.stop_propagation();
        });
        let dragging = state.1.clone();
        let hover_hitbox = hitbox.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            if hover_hitbox.is_hovered(window) != hovered {
                window.refresh();
            }
            let Some(grab) = dragging.grab_offset.get() else {
                return;
            };
            if !event.dragging() {
                dragging.grab_offset.set(None);
            } else {
                dragging.handle.set_offset(point(
                    dragging.handle.offset().x,
                    px(-geometry.scroll_at(event.position.y.into(), grab)),
                ));
                cx.stop_propagation();
            }
            window.refresh();
        });
        let released = state.1.grab_offset.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase.capture() && event.button == MouseButton::Left && released.take().is_some() {
                window.refresh();
                cx.stop_propagation();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Geometry, MIN_THUMB, WHEEL_FADE, WHEEL_FEEDBACK, WHEEL_HOLD, WheelFeedback, wheel_moved,
    };
    use gpui::px;
    use std::time::Instant;

    #[test]
    fn wheel_feedback_is_bounded_and_respects_reduced_motion() {
        let started = Instant::now();
        for animated in [false, true] {
            let feedback = WheelFeedback { started, animated };
            assert_eq!(feedback.intensity(started + WHEEL_HOLD), 1.);
            assert_eq!(
                feedback.intensity(started + WHEEL_HOLD + WHEEL_FADE / 2),
                if animated { 0.5 } else { 1. },
            );
            assert_eq!(feedback.intensity(started + WHEEL_FEEDBACK), 0.);
        }
    }

    #[test]
    fn wheel_stays_inside_nested_content_until_its_boundary() {
        assert!(
            !wheel_moved(px(-100.), px(-140.), px(100.)),
            "an already clamped inner scroll area must let wheel events bubble",
        );
        assert!(wheel_moved(px(-80.), px(-120.), px(100.)));
        assert!(!wheel_moved(px(0.), px(40.), px(100.)));
        assert!(wheel_moved(px(-20.), px(20.), px(100.)));
        assert!(!wheel_moved(px(-40.), px(-80.), px(0.)));
    }

    #[test]
    fn scrollbar_only_appears_for_overflow_and_keeps_a_usable_thumb() {
        assert!(Geometry::new(0., 200., 0., 0.).is_none());
        assert!(Geometry::new(0., 200., 0.01, 0.).is_none());
        assert!(Geometry::new(0., 0., 200., 0.).is_none());
        let bar = Geometry::new(10., 200., 600., 0.).unwrap();
        assert_eq!(bar.track_top, 12.);
        assert_eq!(bar.thumb_height, 49.);
        assert_eq!(bar.thumb_top, 12.);
        let long = Geometry::new(0., 200., 1_000_000., 0.).unwrap();
        assert_eq!(long.thumb_height, MIN_THUMB);
        let short = Geometry::new(0., 12., 100., 0.).unwrap();
        assert!(short.thumb_height <= short.track_height);
        assert!(short.track_height - short.thumb_height > 0.5);
    }

    #[test]
    fn dragging_and_track_clicks_reach_both_ends_without_jumping() {
        let bar = Geometry::new(10., 200., 600., -300.).unwrap();
        let grab = 11.;
        assert_eq!(bar.scroll_at(bar.thumb_top + grab, grab), 300.);
        assert_eq!(bar.scroll_at(-100., grab), 0.);
        assert_eq!(bar.scroll_at(1000., grab), 600.);
        let end = Geometry::new(10., 200., 600., -600.).unwrap();
        assert_eq!(
            end.thumb_top + end.thumb_height,
            end.track_top + end.track_height
        );
        let resized = Geometry::new(10., 200., 100., -600.).unwrap();
        assert_eq!(
            resized.thumb_top + resized.thumb_height,
            resized.track_top + resized.track_height
        );
    }
}
