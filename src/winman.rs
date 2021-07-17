use log::{debug, info, trace, warn};
use std::collections::HashMap;
use std::rc::Rc;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::{Command, KeybindAction};

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
    ErrorKind,
};
use x11rb::x11_utils::X11Error;
use Window as Wid;

#[derive(Debug, Clone)]
struct Context<C: Connection> {
    conn: Rc<C>,
    config: Rc<Config>,
    root: Wid,
}

#[derive(Debug)]
struct WindowState {
    mapped: bool,
}

#[derive(Debug)]
pub struct WinMan<C: Connection> {
    ctx: Context<C>,
    windows: HashMap<Wid, WindowState>,
    monitor_size: (u16, u16),
    border_visible: bool,
}

impl<C: Connection> WinMan<C> {
    pub fn new(conn: Rc<C>, config: Rc<Config>, root: Wid) -> Result<Self> {
        let mut wm = Self {
            ctx: Context { conn, config, root },
            windows: HashMap::new(),
            monitor_size: (0, 0),
            border_visible: false,
        };
        wm.init()?;

        Ok(wm)
    }

    fn init(&mut self) -> Result<()> {
        // Become a window manager of the root window.
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let aux = ChangeWindowAttributesAux::new().event_mask(mask);
        self.ctx
            .conn
            .change_window_attributes(self.ctx.root, &aux)?
            .check()
            .map_err(|_| Error::WmAlreadyExists)?;

        // Grab keys
        for (&(_, modif, keycode), _) in self.ctx.config.keybind_iter() {
            self.ctx
                .conn
                .grab_key(
                    true,
                    self.ctx.root,
                    modif,
                    keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()
                .map_err(|_| Error::KeyAlreadyGrabbed)?;
        }

        // Grab mouse buttons
        let event_mask: u32 = (EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE).into();
        self.ctx
            .conn
            .grab_button(
                false,
                self.ctx.root,
                event_mask as u16,
                GrabMode::SYNC,
                GrabMode::ASYNC,
                self.ctx.root,
                x11rb::NONE,
                ButtonIndex::M1,
                ModMask::ANY,
            )?
            .check()
            .map_err(|_| Error::ButtonAlreadyGrabbed)?;

        // Receive RROutputChangeNotifyEvent
        self.ctx
            .conn
            .randr_select_input(self.ctx.root, randr::NotifyMask::OUTPUT_CHANGE)?;

        self.refresh_monitor()?;

        // List all exising windows
        let preexist = self.ctx.conn.query_tree(self.ctx.root)?.reply()?.children;

        // Configure existing windows
        for wid in preexist {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;
            let state = WindowState {
                mapped: attr.map_state == MapState::VIEWABLE,
            };
            self.windows.insert(wid, state);
        }

        debug!("windows = {:?}", self.windows.keys());
        self.refresh_layout_horizontal_split()?;

        Ok(())
    }

    fn refresh_monitor(&mut self) -> Result<()> {
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        let mon = monitors_reply.monitors.get(0).expect("no monitor");
        self.monitor_size = (mon.width, mon.height);
        Ok(())
    }

    fn refresh_layout_horizontal_split(&mut self) -> Result<()> {
        let mapped_count = self
            .windows
            .iter()
            .filter(|(_, state)| state.mapped)
            .count();
        if mapped_count == 0 {
            return Ok(());
        }

        let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
        debug!("focused = {}", focused);

        let w = (self.monitor_size.0 / mapped_count as u16) as u32;
        let h = self.monitor_size.1 as u32;
        let mut x = 0;

        for (&wid, _) in self.windows.iter().filter(|(_, state)| state.mapped) {
            let show_border = self.border_visible && wid == focused;

            let border = self.ctx.config.border.clone();
            let conf = ConfigureWindowAux::new()
                .x(x)
                .y(0)
                .border_width(border.width)
                .width(w - border.width * 2)
                .height(h - border.width * 2);
            self.ctx.conn.configure_window(wid, &conf)?;

            if show_border {
                let attr = ChangeWindowAttributesAux::new().border_pixel(border.color_focused);
                self.ctx.conn.change_window_attributes(wid, &attr)?;
            } else {
                let attr = ChangeWindowAttributesAux::new().border_pixel(border.color_regular);
                self.ctx.conn.change_window_attributes(wid, &attr)?;
            }

            x += w as i32;
        }
        self.ctx.conn.flush()?;
        Ok(())
    }

    fn map_window(&mut self, wid: Wid) -> Result<()> {
        let state = self.windows.get_mut(&wid).expect("unknown window");
        state.mapped = true;
        self.refresh_layout_horizontal_split()?;
        Ok(())
    }

    fn unmap_window(&mut self, wid: Wid) -> Result<()> {
        let state = self.windows.get_mut(&wid).expect("unknown window");
        state.mapped = false;

        let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
        debug!("unmap_window: current focus = {}", focused);
        if focused == InputFocus::POINTER_ROOT.into() {
            self.focus_next()?;
        }

        self.refresh_layout_horizontal_split()?;
        Ok(())
    }

    fn focus_next(&mut self) -> Result<()> {
        let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
        let mut iter = self.windows.iter().filter(|(_, st)| st.mapped);
        let next = iter
            .clone()
            .skip_while(|(&wid, _)| wid != focused)
            .nth(1)
            .or_else(|| iter.next())
            .map(|(wid, _)| wid);

        debug!("focus_next: current={} --> next={:?}", focused, next);

        if let Some(&next) = next {
            match self
                .ctx
                .conn
                .set_input_focus(InputFocus::POINTER_ROOT, next, x11rb::CURRENT_TIME)?
                .check()
            {
                Ok(()) => {}
                Err(ReplyError::X11Error(X11Error {
                    error_kind: ErrorKind::Window,
                    ..
                })) => {
                    warn!("the next window (id={}) not found", next);
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),
            Command::ShowBorder => {
                self.border_visible = true;
                self.refresh_layout_horizontal_split()?;
            }
            Command::HideBorder => {
                self.border_visible = false;
                self.refresh_layout_horizontal_split()?;
            }
            Command::Close => {
                let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
                self.ctx.conn.destroy_window(focused)?;
                self.ctx.conn.flush()?;
            }
            Command::FocusNext => {
                self.focus_next()?;
                self.refresh_layout_horizontal_split()?;
            }
            Command::FocusPrev => {
                warn!("not yet implemented");
            }
            Command::OpenLauncher => {
                let _ = std::process::Command::new(self.ctx.config.launcher.as_str()).spawn();
            }
        }
        Ok(())
    }
}

impl<C: Connection> EventHandlerMethods for WinMan<C> {
    fn on_key_press(&mut self, e: KeyPressEvent) -> Result<HandleResult> {
        if let Some(cmd) = self
            .ctx
            .config
            .keybind_match(KeybindAction::Press, e.state, e.detail)
        {
            debug!("on_key_press: cmd = {:?}", cmd);
            self.process_command(cmd)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_key_release(&mut self, e: KeyReleaseEvent) -> Result<HandleResult> {
        if let Some(cmd) = self
            .ctx
            .config
            .keybind_match(KeybindAction::Release, e.state, e.detail)
        {
            debug!("on_key_release: cmd = {:?}", cmd);
            self.process_command(cmd)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_button_press(&mut self, e: ButtonPressEvent) -> Result<HandleResult> {
        self.ctx
            .conn
            .allow_events(Allow::REPLAY_POINTER, x11rb::CURRENT_TIME)?
            .check()?;
        if e.child != x11rb::NONE {
            debug!("set_input_focus");
            self.ctx.conn.set_input_focus(
                InputFocus::POINTER_ROOT,
                e.child,
                x11rb::CURRENT_TIME,
            )?;
            self.ctx.conn.flush()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        self.ctx.conn.map_window(req.window)?;
        self.ctx.conn.flush()?;
        Ok(HandleResult::Consumed)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            self.map_window(notif.window)?;
            self.ctx.conn.set_input_focus(
                InputFocus::POINTER_ROOT,
                notif.window,
                x11rb::CURRENT_TIME,
            )?;
            self.ctx.conn.flush()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if self.windows.contains_key(&notif.window) {
            self.unmap_window(notif.window)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let state = WindowState { mapped: false };
            self.windows.insert(notif.window, state);
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        if self.windows.contains_key(&notif.window) {
            self.windows.remove(&notif.window);
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }
}
