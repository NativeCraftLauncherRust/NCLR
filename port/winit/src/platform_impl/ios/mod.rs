#![allow(clippy::let_unit_value)]

mod app_state;
mod event_loop;
pub(crate) mod hardware_input;
mod monitor;
mod notification_center;
mod view;
mod view_controller;
mod window;

use std::fmt;

use crate::event::DeviceId as RootDeviceId;

pub(crate) use self::event_loop::{
    ActiveEventLoop, EventLoop, EventLoopProxy, OwnedDisplayHandle,
    PlatformSpecificEventLoopAttributes,
};
pub(crate) use self::monitor::{MonitorHandle, VideoModeHandle};
pub(crate) use self::window::{PlatformSpecificWindowAttributes, Window, WindowId};
pub(crate) use crate::cursor::{
    NoCustomCursor as PlatformCustomCursor, NoCustomCursor as PlatformCustomCursorSource,
};
pub(crate) use crate::icon::NoIcon as PlatformIcon;
pub(crate) use crate::platform_impl::Fullscreen;

/// There is no way to detect which device that performed a certain event in
/// UIKit (i.e. you can't differentiate between different external keyboards,
/// or whether it was the main touchscreen, assistive technologies, or some
/// other pointer device that caused a touch event).
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId;

impl DeviceId {
    pub const fn dummy() -> Self {
        DeviceId
    }
}

pub(crate) const DEVICE_ID: RootDeviceId = RootDeviceId(DeviceId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEventExtra {}

#[derive(Debug)]
pub enum OsError {}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "os error")
    }
}

/// Callbacks the UIKit shell registers so it can own the text-input view: the
/// engine's `set_ime_allowed` then asks the shell to show/hide its IME-capable
/// text view, instead of making winit's plain key-input view first responder
/// (which cannot compose Chinese).
static IME_HOOKS: std::sync::Mutex<Option<(extern "C" fn(), extern "C" fn())>> =
    std::sync::Mutex::new(None);

pub fn set_ime_hooks(begin: Option<extern "C" fn()>, end: Option<extern "C" fn()>) {
    if let Ok(mut hooks) = IME_HOOKS.lock() {
        *hooks = begin.zip(end);
    }
}

/// Runs the shell's begin/end hook for the requested IME state. True when a
/// hook took the request, so winit must not touch the responder chain.
pub fn run_ime_hook(allowed: bool) -> bool {
    let hooks = IME_HOOKS.lock().ok().and_then(|hooks| *hooks);
    let Some((begin, end)) = hooks else {
        return false;
    };
    if allowed {
        begin();
    } else {
        end();
    }
    true
}
