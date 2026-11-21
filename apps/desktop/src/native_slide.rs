//! Position the existing panel from its Win32 message loop, outside GPUI borrows.
use std::{
    cell::Cell,
    mem::size_of,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};

pub(super) const TIMER_ID: usize = 0x574c;
const FULL_SLIDE_MS: f64 = 200.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Surface {
    #[default]
    Hidden,
    DropTarget,
    Panel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    monitor: HMONITOR,
    dpi: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    screen_right: i32,
}

#[derive(Clone, Copy, Debug)]
struct Motion {
    panel: Placement,
    hidden: Placement,
    begin: Instant,
    duration: Duration,
    from: i32,
    to: i32,
    last_x: i32,
}

impl Motion {
    fn new(panel: Placement, hidden: Placement, begin: Instant, from: i32, to: i32) -> Self {
        let distance = f64::from(from.abs_diff(to));
        let full_distance = f64::from(panel.x.abs_diff(panel.screen_right)).max(1.);
        let ms = (FULL_SLIDE_MS * distance / full_distance).clamp(55., FULL_SLIDE_MS);
        Self {
            panel,
            hidden,
            begin,
            duration: Duration::from_secs_f64(ms / 1000.),
            from,
            to,
            last_x: from,
        }
    }

    fn sample(self, now: Instant) -> (i32, bool) {
        let t = (now.saturating_duration_since(self.begin).as_secs_f64()
            / self.duration.as_secs_f64())
        .min(1.);
        let eased = t * t * (3. - 2. * t);
        let x = f64::from(self.from) + (f64::from(self.to) - f64::from(self.from)) * eased;
        (x.round() as i32, t >= 1.)
    }
}

#[derive(Clone, Copy, Default)]
struct SlideState {
    target: Surface,
    motion: Option<Motion>,
    transition: usize,
}

thread_local! {
    // Only the sharing window installs this subclass, on its owning UI thread.
    static SLIDE: Cell<SlideState> = Cell::new(SlideState::default());
}

struct Screen {
    handle: HMONITOR,
    work: RECT,
    screen_right: i32,
    dpi: u32,
}

impl Screen {
    fn read(hwnd: HWND, previous: Option<Motion>) -> Option<Self> {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // Keep the animation on its starting monitor even when the majority of
        // the off-screen HWND overlaps the neighboring monitor.
        let handle = unsafe {
            if let Some(motion) = previous
                && GetMonitorInfoW(motion.panel.monitor, &mut info).as_bool()
            {
                return Some(Self {
                    handle: motion.panel.monitor,
                    work: info.rcWork,
                    screen_right: info.rcMonitor.right,
                    dpi: motion.panel.dpi,
                });
            }
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
        };
        if !unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
            return None;
        }
        Some(Self {
            handle,
            work: info.rcWork,
            screen_right: info.rcMonitor.right,
            dpi: unsafe { GetDpiForWindow(hwnd) }.max(96),
        })
    }

    fn placement(&self, surface: Surface) -> Placement {
        let (width, height) = if surface == Surface::Panel {
            (356., 476.)
        } else {
            (200., 56.)
        };
        let scale = self.dpi as f32 / 96.;
        let width = (width * scale).round() as i32;
        let height = (height * scale).round() as i32;
        let margin = (12. * scale).round() as i32;
        Placement {
            monitor: self.handle,
            dpi: self.dpi,
            width,
            height,
            screen_right: self.screen_right,
            x: (self.work.right - width - margin).max(self.work.left),
            y: (self.work.bottom - height - margin).max(self.work.top),
        }
    }
}

/// Returns true when a collapsed request has finished hiding a panel.
pub(super) fn place(
    hwnd: HWND,
    expanded: bool,
    visible: bool,
    activate: bool,
    transition: usize,
) -> bool {
    let target = if !visible {
        Surface::Hidden
    } else if expanded {
        Surface::Panel
    } else {
        Surface::DropTarget
    };
    let previous = SLIDE.with(Cell::get);
    let panel_was_visible = previous.target == Surface::Panel || previous.motion.is_some();
    let Some(screen) = Screen::read(hwnd, previous.motion) else {
        if target == Surface::Hidden {
            stop(hwnd);
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            return panel_was_visible;
        }
        return false;
    };
    let panel = screen.placement(Surface::Panel);
    let hidden = screen.placement(Surface::Hidden);
    let animation = crate::scroll_area::client_area_animation_enabled();
    if animation
        && previous.target == target
        && previous.transition == transition
        && previous
            .motion
            .is_some_and(|motion| motion.hidden == hidden)
    {
        if target == Surface::Panel && activate {
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
        }
        return false;
    }

    // Scope native show/hide and sizing to this posted message, not the GPUI render callback.
    let already_visible = unsafe { IsWindowVisible(hwnd).as_bool() };
    let direct = match target {
        Surface::Hidden => !panel_was_visible || !already_visible,
        Surface::DropTarget => true,
        Surface::Panel => {
            already_visible && previous.target != Surface::Hidden && previous.motion.is_none()
        }
    };
    if !animation || direct {
        settle(
            hwnd,
            target,
            if expanded && visible { panel } else { hidden },
            activate,
            transition,
        );
        return target == Surface::Hidden && panel_was_visible;
    }

    let mut current = RECT::default();
    let live_rect = already_visible && unsafe { GetWindowRect(hwnd, &mut current) }.is_ok();
    let from = if live_rect {
        current.left
    } else {
        panel.screen_right
    };
    let to = if target == Surface::Hidden {
        panel.screen_right
    } else {
        panel.x
    };
    let panel = if target == Surface::Hidden && live_rect {
        Placement {
            y: current.top,
            width: current.right - current.left,
            height: current.bottom - current.top,
            ..panel
        }
    } else {
        panel
    };
    if from == to || unsafe { SetTimer(Some(hwnd), TIMER_ID, 15, None) } == 0 {
        settle(
            hwnd,
            target,
            if target == Surface::Panel {
                panel
            } else {
                hidden
            },
            activate,
            transition,
        );
        return target == Surface::Hidden && panel_was_visible;
    }
    let motion = Motion::new(panel, hidden, Instant::now(), from, to);
    SLIDE.with(|state| {
        state.set(SlideState {
            target,
            motion: Some(motion),
            transition,
        })
    });
    if target == Surface::Panel {
        crate::window_frame::configure(hwnd, panel.width, panel.height);
        clip_panel(hwnd, panel, from);
        size_panel(hwnd, panel, from);
        show(hwnd, activate);
    }
    false
}

pub(super) fn tick(hwnd: HWND) -> Option<usize> {
    let mut state = SLIDE.with(Cell::get);
    let mut motion = state.motion?;
    let (x, done) = motion.sample(Instant::now());
    if done {
        let closing = state.target == Surface::Hidden;
        settle(
            hwnd,
            state.target,
            if closing { motion.hidden } else { motion.panel },
            false,
            state.transition,
        );
        return closing.then_some(state.transition);
    }
    if motion.last_x != x {
        if x > motion.last_x
            && visible_width(motion.panel, x) != visible_width(motion.panel, motion.last_x)
        {
            clip_panel(hwnd, motion.panel, x);
        }
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                x,
                motion.panel.y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE | SWP_NOOWNERZORDER,
            );
        }
        if x < motion.last_x
            && visible_width(motion.panel, x) != visible_width(motion.panel, motion.last_x)
        {
            clip_panel(hwnd, motion.panel, x);
        }
        motion.last_x = x;
        state.motion = Some(motion);
        SLIDE.with(|cell| cell.set(state));
    }
    None
}

fn visible_width(panel: Placement, x: i32) -> i32 {
    (panel.screen_right - x).clamp(0, panel.width)
}

fn clip_panel(hwnd: HWND, panel: Placement, x: i32) {
    crate::window_frame::clip_right(hwnd, panel.width, panel.height, visible_width(panel, x));
}

fn settle(hwnd: HWND, target: Surface, placement: Placement, activate: bool, transition: usize) {
    SLIDE.with(|state| {
        state.set(SlideState {
            target,
            motion: None,
            transition,
        })
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_ID);
    }
    if target == Surface::Hidden {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    crate::window_frame::configure(hwnd, placement.width, placement.height);
    size_panel(hwnd, placement, placement.x);
    if target != Surface::Hidden {
        show(hwnd, activate);
    }
}

fn size_panel(hwnd: HWND, placement: Placement, x: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            placement.y,
            placement.width,
            placement.height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
        );
    }
}

fn show(hwnd: HWND, activate: bool) {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        if activate {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub(super) fn is_closing() -> bool {
    let state = SLIDE.with(Cell::get);
    state.target == Surface::Hidden && state.motion.is_some()
}

pub(super) fn begin_drag(hwnd: HWND) {
    let state = SLIDE.with(Cell::get);
    if let Some(motion) = state.motion
        && state.target == Surface::Panel
    {
        SLIDE.with(|cell| {
            cell.set(SlideState {
                motion: None,
                ..state
            })
        });
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
        }
        crate::window_frame::clip_right(
            hwnd,
            motion.panel.width,
            motion.panel.height,
            motion.panel.width,
        );
    }
}

pub(super) fn stop(hwnd: HWND) {
    SLIDE.with(|state| state.set(SlideState::default()));
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_ID);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_continues_from_the_last_position_and_lands_at_the_edge() {
        let now = Instant::now();
        let panel = Placement {
            monitor: HMONITOR::default(),
            dpi: 96,
            x: 1552,
            y: 592,
            width: 356,
            height: 476,
            screen_right: 1920,
        };
        let incoming = Motion::new(panel, panel, now, panel.screen_right, panel.x);
        let half = now + incoming.duration / 2;
        let (x, done) = incoming.sample(half);
        assert!(!done);
        assert_eq!(x, 1736);
        assert_eq!(visible_width(panel, x), 184);
        let outgoing = Motion::new(panel, panel, half, x, panel.screen_right);
        assert_eq!(outgoing.sample(half), (x, false));
        assert_eq!(
            outgoing.sample(half + outgoing.duration),
            (panel.screen_right, true)
        );
        assert_eq!(visible_width(panel, panel.x), panel.width);
        assert_eq!(visible_width(panel, panel.screen_right), 0);
    }
}
