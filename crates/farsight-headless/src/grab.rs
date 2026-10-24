//! Moving a window while the left button is held (clients ask for this when
//! their title bar is dragged).

use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::state::State;

/// Linux `BTN_LEFT`.
pub(crate) const BTN_LEFT: u32 = 0x110;

pub(crate) struct MoveGrab {
    pub start_data: GrabStartData<State>,
    pub window: Window,
    pub initial_location: Point<i32, Logical>,
}

type Handle<'a> = PointerInnerHandle<'a, State>;
type Focus = Option<(WlSurface, Point<f64, Logical>)>;

impl PointerGrab<State> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        _focus: Focus,
        event: &MotionEvent,
    ) {
        // No client has pointer focus while a window is being moved.
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;
        let location = (self.initial_location.to_f64() + delta).to_i32_round();
        data.move_window(&self.window, location);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        focus: Focus,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut State, handle: &mut Handle<'_>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut State, handle: &mut Handle<'_>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut Handle<'_>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut Handle<'_>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut State) {}
}
