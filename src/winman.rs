use log::{debug, info, trace, warn};
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::{Command, Config, KeybindAction};

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
};
use Window as Wid;

#[derive(Debug)]
struct WindowState {
    mapped: bool,
}

#[derive(Debug)]
pub struct WinMan<C: Connection> {
    conn: Rc<C>,
    config: Rc<Config>,
    root: Wid,
    windows: HashMap<Wid, WindowState>,
    monitor_size: (u16, u16),
    border_visible: bool,
}

impl<C: Connection> WinMan<C> {
    pub fn new(conn: Rc<C>, config: Rc<Config>, root: Wid) -> Result<Self> {
        let mut wm = Self {
            conn,
            config,
            root,
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
        self.conn
            .change_window_attributes(self.root, &aux)?
            .check()
            .map_err(|_| Error::WmAlreadyExists)?;

        // Grab keys
        for (_, modif, keycode) in self.config.bounded_keys() {
            self.conn
                .grab_key(
                    true,
                    self.root,
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
        self.conn
            .grab_button(
                false,
                self.root,
                event_mask as u16,
                GrabMode::SYNC,
                GrabMode::ASYNC,
                self.root,
                x11rb::NONE,
                ButtonIndex::M1,
                ModMask::ANY,
            )?
            .check()
            .map_err(|_| Error::ButtonAlreadyGrabbed)?;

        // Receive RROutputChangeNotifyEvent
        self.conn
            .randr_select_input(self.root, randr::NotifyMask::OUTPUT_CHANGE)?;

        self.refresh_monitor()?;

        // List all exising windows
        let preexist = self.conn.query_tree(self.root)?.reply()?.children;

        // Configure existing windows
        for wid in preexist {
            let attr = self.conn.get_window_attributes(wid)?.reply()?;
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
        let monitors_reply = self.conn.randr_get_monitors(self.root, true)?.reply()?;
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

        let focused = self.conn.get_input_focus()?.reply()?.focus;
        debug!("focused = {}", focused);

        let w = (self.monitor_size.0 / mapped_count as u16) as u32;
        let h = self.monitor_size.1 as u32;
        let mut x = 0;

        for (&wid, _) in self.windows.iter().filter(|(_, state)| state.mapped) {
            let show_border = self.border_visible && wid == focused;
            let mut conf = ConfigureWindowAux::new().x(x).y(0);
            if show_border {
                const BORDER_WIDTH: u32 = 2;
                conf = conf
                    .border_width(BORDER_WIDTH)
                    .width(w - BORDER_WIDTH * 2)
                    .height(h - BORDER_WIDTH * 2);
            } else {
                conf = conf.border_width(0).width(w).height(h)
            }
            self.conn.configure_window(wid, &conf)?;

            if show_border {
                let attr = ChangeWindowAttributesAux::new().border_pixel(0xFF8882);
                self.conn.change_window_attributes(wid, &attr)?;
            }

            x += w as i32;
        }
        self.conn.flush()?;
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
        self.refresh_layout_horizontal_split()?;
        Ok(())
    }

    fn focus_next(&mut self) -> Result<()> {
        let focused = self.conn.get_input_focus()?.reply()?.focus;
        let next = self
            .windows
            .iter()
            .filter(|(_, st)| st.mapped)
            .skip_while(|(&wid, _)| wid != focused)
            .nth(1)
            .or_else(|| self.windows.iter().find(|(_, st)| st.mapped))
            .map(|(wid, _)| wid);
        if let Some(&next) = next {
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, next, x11rb::CURRENT_TIME)?
                .check()?;
        }
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Exit => return Err(Error::Quit),
            Command::ShowBorder => {
                self.border_visible = true;
                self.refresh_layout_horizontal_split()?;
            }
            Command::HideBorder => {
                self.border_visible = false;
                self.refresh_layout_horizontal_split()?;
            }
            Command::Close => {
                let focused = self.conn.get_input_focus()?.reply()?.focus;
                self.focus_next()?;
                self.conn.destroy_window(focused)?;
                self.conn.flush()?;
            }
            Command::FocusNext => {
                self.focus_next()?;
                self.refresh_layout_horizontal_split()?;
            }
            Command::FocusPrev => {
                warn!("not yet implemented");
            }
            Command::OpenLauncher => {
                let _ = std::process::Command::new(self.config.launcher.as_str()).spawn();
            }
        }
        Ok(())
    }
}

impl<C: Connection> EventHandlerMethods for WinMan<C> {
    fn on_key_press(&mut self, e: KeyPressEvent) -> Result<HandleResult> {
        if let Some(cmd) = self
            .config
            .get_keybind(KeybindAction::Press, e.state, e.detail)
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
            .config
            .get_keybind(KeybindAction::Release, e.state, e.detail)
        {
            debug!("on_key_release: cmd = {:?}", cmd);
            self.process_command(cmd)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_button_press(&mut self, e: ButtonPressEvent) -> Result<HandleResult> {
        self.conn
            .allow_events(Allow::REPLAY_POINTER, x11rb::CURRENT_TIME)?
            .check()?;
        if e.child != x11rb::NONE {
            debug!("set_input_focus");
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, e.child, x11rb::CURRENT_TIME)?;
            self.conn.flush()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        self.conn.map_window(req.window)?;
        self.conn.flush()?;
        Ok(HandleResult::Consumed)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            self.map_window(notif.window)?;
            self.conn.set_input_focus(
                InputFocus::POINTER_ROOT,
                notif.window,
                x11rb::CURRENT_TIME,
            )?;
            self.conn.flush()?;
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
