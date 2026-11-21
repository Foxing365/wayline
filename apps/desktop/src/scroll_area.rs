//! Shared scroll views with smooth mouse-wheel detents and direct precision input.
use gpui::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

const GUTTER: f32 = 12.;
const INSET: f32 = 2.;
const MIN_THUMB: f32 = 24.;
const WHEEL_TRAVEL: Duration = Duration::from_millis(145);
const WHEEL_HOLD: Duration = Duration::from_millis(90);
const WHEEL_FADE: Duration = Duration::from_millis(210);
const WHEEL_FEEDBACK: Duration = WHEEL_HOLD.saturating_add(WHEEL_FADE);

#[derive(Clone, Default)]
pub struct ScrollState {
    handle: ScrollHandle,
    grab_offset: Rc<Cell<Option<f32>>>,
    motion: Rc<Cell<Option<WheelMotion>>>,
    wheel_feedback: Rc<Cell<Option<WheelFeedback>>>,
    feedback_expiry: Rc<RefCell<Option<Task<()>>>>,
}

#[derive(Clone, Copy)]
struct WheelMotion {
    started: Instant,
    from: Pixels,
    target: Pixels,
}

impl ScrollState {
    fn scroll_directly(&self, y: Pixels) {
        self.motion.set(None);
        self.handle.set_offset(point(self.handle.offset().x, y));
    }

    /// GPUI has already resolved axes and line height and temporarily applied the
    /// delta. Retain that distance while making the actual content and thumb move together.
    fn wheel(
        &self,
        before: Pixels,
        delta: Pixels,
        max: Pixels,
        smooth: bool,
        now: Instant,
    ) -> bool {
        let native = self.handle.offset();
        if delta == px(0.) {
            return false;
        }
        let visible = before.clamp(-max, px(0.));
        if !smooth {
            let next = (visible + delta).clamp(-max, px(0.));
            self.scroll_directly(next);
            return next != visible;
        }

        self.handle.set_offset(point(native.x, visible));
        let mut base = visible;
        if let Some(motion) = self.motion.get() {
            let target = motion.target.clamp(-max, px(0.));
            if (target - visible) * delta.signum() > px(0.) {
                base = target;
            } else {
                // A reversal starts at the displayed position without traversing
                // the queued distance in the previous direction.
                self.motion.set(None);
            }
        }
        let target = (base + delta).clamp(-max, px(0.));
        if target == base {
            return false; // The next notch belongs to the enclosing scroll area.
        }
        self.motion.set(Some(WheelMotion {
            started: now,
            from: visible,
            target,
        }));
        true
    }

    fn advance(&self, now: Instant, max: Pixels, animated: bool) -> bool {
        let Some(motion) = self.motion.get() else {
            return false;
        };
        let target = motion.target.clamp(-max, px(0.));
        let elapsed = now.saturating_duration_since(motion.started);
        let fraction = if animated && elapsed < WHEEL_TRAVEL {
            let remaining = 1. - elapsed.as_secs_f32() / WHEEL_TRAVEL.as_secs_f32();
            1. - remaining * remaining * remaining
        } else {
            1.
        };
        let y = if fraction >= 1. {
            target
        } else {
            (motion.from + (motion.target - motion.from) * fraction).clamp(-max, px(0.))
        };
        self.handle.set_offset(point(self.handle.offset().x, y));
        if y == target {
            self.motion.set(None);
            return false;
        }
        true
    }
}

fn wheel_lines(
    lines: f32,
    units_per_notch: u32,
    native: Pixels,
    viewport: Pixels,
) -> (Pixels, bool) {
    if units_per_notch == 0 {
        return (native, false);
    }
    let ticks = lines / units_per_notch as f32;
    // GPUI passes Win32's "one screen at a time" sentinel through as a line
    // multiplier. One detent should travel one viewport in that mode.
    let pixels = if units_per_notch == u32::MAX {
        viewport * ticks
    } else {
        native
    };
    (
        pixels,
        ticks.abs() >= 1. && (ticks - ticks.round()).abs() < 0.0001,
    )
}

fn wheel_input(delta: ScrollDelta, native: Pixels, viewport: Pixels) -> (Pixels, bool) {
    use windows::Win32::UI::{
        Input::{
            GetCurrentInputMessageSource, IMDT_PEN, IMDT_TOUCH, IMDT_TOUCHPAD, INPUT_MESSAGE_SOURCE,
        },
        WindowsAndMessaging::{
            SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SystemParametersInfoW,
        },
    };
    let ScrollDelta::Lines(lines) = delta else {
        return (native, false);
    };
    if native == px(0.) {
        return (native, false);
    }
    let mut source = INPUT_MESSAGE_SOURCE::default();
    // SAFETY: dispatch is synchronous on the Win32 message thread; both Win32
    // calls below write into a live value of the type required by their API.
    let direct = unsafe { GetCurrentInputMessageSource(&mut source) }.is_ok()
        && matches!(source.deviceType, IMDT_TOUCHPAD | IMDT_TOUCH | IMDT_PEN);
    // The GPUI Windows backend scales WM_MOUSE[H]WHEEL into lines, including
    // fractional detents from devices delivered through mouse compatibility.
    let (amount, setting) = if lines.y != 0. {
        (lines.y, SPI_GETWHEELSCROLLLINES)
    } else {
        (lines.x, SPI_GETWHEELSCROLLCHARS)
    };
    let mut units = 0u32;
    if unsafe {
        SystemParametersInfoW(
            setting,
            0,
            Some((&raw mut units).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_err()
    {
        return (native, false);
    }
    let (pixels, notch) = wheel_lines(amount, units, native, viewport);
    (pixels, !direct && notch)
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

/// Keeps the div's layout, native wheel distance and identity across renders.
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
        if state.1.motion.get().is_some() {
            state.1.advance(
                Instant::now(),
                state.1.handle.max_offset().height,
                client_area_animation_enabled(),
            );
        }
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
            state.1.motion.set(None);
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
        // Registered before the div: capture its initial offset, then observe
        // GPUI's native delta during reverse-order bubbling, before any frame paints.
        if let Some(hitbox) = &prepaint.0 {
            let wheel_hitbox = hitbox.clone();
            let scroll = state.1.clone();
            let previous_offset = Rc::new(Cell::new(None));
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if !wheel_hitbox.should_handle_scroll(window) {
                    return;
                }
                if phase.capture() {
                    previous_offset.set(Some(scroll.handle.offset().y));
                } else if phase.bubble()
                    && let Some(previous) = previous_offset.take()
                {
                    let now = Instant::now();
                    let animated = client_area_animation_enabled();
                    let (delta, notch) = wheel_input(
                        event.delta,
                        scroll.handle.offset().y - previous,
                        scroll.handle.bounds().size.height,
                    );
                    let smooth = animated && scroll.grab_offset.get().is_none() && notch;
                    if !scroll.wheel(
                        previous,
                        delta,
                        scroll.handle.max_offset().height,
                        smooth,
                        now,
                    ) {
                        return;
                    }
                    if scroll.grab_offset.get().is_none() {
                        scroll.wheel_feedback.set(Some(WheelFeedback {
                            started: now,
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
        let mut needs_frame = state.1.motion.get().is_some();
        let mut intensity = 0.;
        let mut width = if direct { 8. } else { 6. };
        if let Some(feedback) = state.1.wheel_feedback.get() {
            intensity = feedback.intensity(Instant::now());
            if intensity == 0. {
                state.1.wheel_feedback.set(None);
            } else if feedback.animated {
                width += (8. - width) * intensity;
                needs_frame = true;
            }
        }
        if needs_frame {
            window.request_animation_frame();
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
            down.scroll_directly(px(-geometry.scroll_at(y, grab)));
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
                dragging.scroll_directly(px(-geometry.scroll_at(event.position.y.into(), grab)));
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
        Geometry, MIN_THUMB, ScrollState, WHEEL_FADE, WHEEL_FEEDBACK, WHEEL_HOLD, WHEEL_TRAVEL,
        WheelFeedback, wheel_input, wheel_lines,
    };
    use gpui::{ScrollDelta, point, px};
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

    // GPUI's native wheel listener runs before our bubble listener.
    fn wheel(state: &ScrollState, delta: f32, max: f32, smooth: bool, now: Instant) -> bool {
        let before = state.handle.offset();
        state
            .handle
            .set_offset(point(before.x, before.y + px(delta)));
        state.wheel(before.y, px(delta), px(max), smooth, now)
    }

    #[test]
    fn wheel_updates_the_actual_scroll_handle_across_frames_and_chains_at_the_edge() {
        let start = Instant::now();
        let inner = ScrollState::default();
        inner.handle.set_offset(point(px(7.), px(0.)));
        assert!(wheel(&inner, -45., 100., true, start));
        assert_eq!(inner.handle.offset(), point(px(7.), px(0.)));
        assert!(inner.advance(start + WHEEL_TRAVEL / 4, px(100.), true));
        let first = inner.handle.offset().y;
        assert!(px(-45.) < first && first < px(0.));
        assert!(inner.advance(start + WHEEL_TRAVEL / 2, px(100.), true));
        let halfway = inner.handle.offset().y;
        assert!(px(-45.) < halfway && halfway < first);
        assert!(wheel(&inner, -70., 100., true, start + WHEEL_TRAVEL / 2));
        assert_eq!(inner.handle.offset().y, halfway);
        // The inner area has already claimed all the remaining distance. The
        // next notch bubbles to the parent even while the inner area settles.
        assert!(!wheel(&inner, -45., 100., true, start + WHEEL_TRAVEL / 2));
        let outer = ScrollState::default();
        assert!(wheel(&outer, -45., 200., true, start + WHEEL_TRAVEL / 2));
        assert!(!inner.advance(start + WHEEL_TRAVEL * 2, px(100.), true));
        assert_eq!(inner.handle.offset(), point(px(7.), px(-100.)));
        assert!(!outer.advance(start + WHEEL_TRAVEL * 2, px(200.), true));
        assert_eq!(outer.handle.offset().y, px(-45.));
    }

    #[test]
    fn reversal_precise_input_reduced_motion_and_resize_interrupt_travel() {
        let start = Instant::now();
        let state = ScrollState::default();
        assert!(wheel(&state, -60., 200., true, start));
        let next = start + WHEEL_TRAVEL / 4;
        assert!(state.advance(next, px(200.), true));
        let before = state.handle.offset().y;
        assert!(wheel(&state, 10., 200., true, next));
        assert!(!state.advance(next + WHEEL_TRAVEL, px(200.), true));
        assert_eq!(state.handle.offset().y, before + px(10.));

        assert!(wheel(&state, -60., 200., true, next));
        assert!(wheel(&state, 2.5, 200., false, next));
        assert_eq!(state.handle.offset().y, before + px(12.5));
        assert!(!state.advance(next + WHEEL_TRAVEL, px(200.), true));
        assert!(wheel(&state, -120., 200., true, next));
        assert!(!state.advance(next, px(200.), false));
        assert_eq!(state.handle.offset().y, before + px(12.5) - px(120.));
        assert!(wheel(&state, -120., 200., true, next));
        assert!(!state.advance(next + WHEEL_TRAVEL / 2, px(15.), true));
        assert_eq!(state.handle.offset().y, px(-15.));
        assert!(wheel(&state, 10., 15., true, next));
        state.scroll_directly(px(-12.));
        assert!(!state.advance(next + WHEEL_TRAVEL, px(15.), true));
        assert_eq!(state.handle.offset().y, px(-12.));
    }

    #[test]
    fn trackpad_pixels_and_partial_windows_detents_are_direct() {
        assert_eq!(
            wheel_input(
                ScrollDelta::Pixels(point(px(0.), px(-24.))),
                px(-24.),
                px(250.)
            ),
            (px(-24.), false)
        );
        let lines = |amount, units| wheel_lines(amount, units, px(-42.), px(250.));
        assert_eq!(lines(-3., 3), (px(-42.), true));
        assert!(lines(12., 3).1);
        assert!(lines(-1., 1).1);
        assert!(!lines(1., 3).1);
        assert!(!lines(1.5, 3).1);
        assert!(!lines(0., 3).1);
        assert!(!lines(1., 0).1);
        assert_eq!(lines(-(u32::MAX as f32), u32::MAX), (px(-250.), true));
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
