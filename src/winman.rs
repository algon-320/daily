use log::{debug, info, warn};
use std::collections::hash_map::{Entry as HashMapEntry, HashMap};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::screen::{Screen, WindowState};
use crate::{Command, KeybindAction};

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
};
use Window as Wid;

#[derive()]
struct Monitor {
    info: randr::MonitorInfo,
    screen: Box<Screen>,
}

#[derive()]
pub struct WinMan {
    ctx: Context,
    monitors: Vec<Monitor>,
    screens: HashMap<usize, Box<Screen>>,
    focused_monitor: usize,
}

impl WinMan {
    pub fn new(ctx: Context) -> Result<Self> {
        let mut wm = Self {
            ctx,
            monitors: Vec::new(),
            screens: HashMap::new(),
            focused_monitor: 0,
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

        // Receive RROutputChangeNotify / RRCrtcChangeNotify
        self.ctx.conn.randr_select_input(
            self.ctx.root,
            randr::NotifyMask::OUTPUT_CHANGE | randr::NotifyMask::CRTC_CHANGE,
        )?;

        // Get currently existing windows
        let preexist = self.ctx.conn.query_tree(self.ctx.root)?.reply()?.children;
        info!("preexist windows = {:?}", &preexist);

        // Setup monitors
        self.setup_monitor()?;

        let first_mon = self.monitors.get_mut(0).expect("no monitor");
        let first = &mut first_mon.screen;

        // Put all pre-existing windows on the first screen.
        for &wid in preexist.iter() {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;

            let state = if attr.map_state == MapState::VIEWABLE {
                WindowState::Mapped
            } else {
                WindowState::Unmapped
            };

            first.add_window(wid, state);
        }

        // Focus the first window
        if let Some(&wid) = preexist.first() {
            self.change_focus(wid)?;
        }

        self.focused_monitor = 0;

        self.refresh_layout()?;
        self.ctx.conn.flush()?;

        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;

        for mon in self.monitors.drain(..) {
            let screen = mon.screen;
            self.screens.insert(screen.id, screen);
        }
        for (id, info) in monitors_reply.monitors.iter().enumerate() {
            if let HashMapEntry::Vacant(entry) = self.screens.entry(id) {
                let screen = Screen::new(self.ctx.clone(), id, info.clone())?;
                entry.insert(Box::new(screen));
            }
        }

        self.monitors = monitors_reply
            .monitors
            .into_iter()
            .enumerate()
            .map(|(id, info)| {
                let mut screen = self.screens.remove(&id).unwrap();
                screen.attach(info.clone())?;
                Ok(Monitor { info, screen })
            })
            .collect::<Result<_>>()?;

        if self.monitors.is_empty() {
            return Err(Error::NoMonitor);
        }

        // TODO: setup additional screens

        Ok(())
    }

    fn refresh_layout(&mut self) -> Result<()> {
        for mon in self.monitors.iter_mut() {
            mon.screen.refresh_layout()?;
        }
        Ok(())
    }

    fn container_of_mut(&mut self, wid: Wid) -> Option<&mut Screen> {
        for mon in self.monitors.iter_mut() {
            if mon.screen.contains(wid).is_some() {
                return Some(&mut mon.screen);
            }
        }

        for screen in self.screens.values_mut() {
            if screen.contains(wid).is_some() {
                return Some(screen);
            }
        }

        None
    }

    // TODO: dirty
    fn container_of_pointer_mut(&mut self) -> Result<&mut Screen> {
        let pointer = self.ctx.conn.query_pointer(self.ctx.root)?.reply()?;
        let (x, y) = (pointer.root_x, pointer.root_y);

        let mon = self
            .monitors
            .iter_mut()
            .find(|mon| {
                let xmn = mon.info.x;
                let xmx = mon.info.x + mon.info.width as i16;
                let ymn = mon.info.y;
                let ymx = mon.info.y + mon.info.height as i16;
                xmn <= x && x < xmx && ymn <= y && y < ymx
            })
            .unwrap();

        info!("pointer on {:?}", mon.info);
        Ok(&mut mon.screen)
    }

    fn change_focus(&mut self, wid: Wid) -> Result<()> {
        debug!("set_input_focus");
        self.ctx
            .conn
            .set_input_focus(InputFocus::POINTER_ROOT, wid, x11rb::CURRENT_TIME)?;
        self.ctx.conn.flush()?;

        self.focus_changed()?;
        Ok(())
    }

    fn focus_changed(&mut self) -> Result<()> {
        let focused_win = self.ctx.get_focused_window()?;
        self.focused_monitor = self
            .monitors
            .iter()
            .position(|mon| mon.screen.contains(focused_win).is_some())
            .unwrap();
        self.refresh_layout()?;
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),

            Command::ShowBorder => {
                for mon in self.monitors.iter_mut() {
                    mon.screen.show_border();
                }
                for screen in self.screens.values_mut() {
                    screen.show_border();
                }
                self.refresh_layout()?;
            }
            Command::HideBorder => {
                for mon in self.monitors.iter_mut() {
                    mon.screen.hide_border();
                }
                for screen in self.screens.values_mut() {
                    screen.hide_border();
                }
                self.refresh_layout()?;
            }

            Command::Close => {
                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    self.ctx.conn.destroy_window(focused)?;
                    self.ctx.conn.flush()?;
                }
            }

            Command::FocusNext => {
                let mon = self.monitors.get_mut(self.focused_monitor).unwrap();
                mon.screen.focus_next()?;
                self.focus_changed()?;
            }
            Command::FocusPrev => {
                warn!("Command::FocusPrev: not yet implemented");
            }

            Command::FocusNextMonitor => {
                // FIXME
                self.focused_monitor = (self.focused_monitor + 1) % self.monitors.len();
                let next_mon = self.monitors.get_mut(self.focused_monitor).unwrap();
                next_mon.screen.focus_any()?;
                self.focus_changed()?;
            }
            Command::FocusPrevMonitor => {
                warn!("Command::FocusPrevMonitor: not yet implemented");
            }

            Command::OpenLauncher => {
                let _ = std::process::Command::new(self.ctx.config.launcher.as_str()).spawn();
            }
        }
        Ok(())
    }
}

impl EventHandlerMethods for WinMan {
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
            self.change_focus(e.child)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let color = self.ctx.config.border.color_regular;
            let attr = ChangeWindowAttributesAux::new().border_pixel(color);
            self.ctx
                .conn
                .change_window_attributes(notif.window, &attr)?;
            self.ctx.conn.flush()?;

            let mon = self.monitors.get_mut(self.focused_monitor).unwrap();
            mon.screen.add_window(notif.window, WindowState::Unmapped);

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
            self.change_focus(notif.window)?;

            if let Some(screen) = self.container_of_mut(notif.window) {
                return screen.on_map_notify(notif);
            }
        }
        Ok(HandleResult::Ignored)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(notif.window) {
            return screen.on_unmap_notify(notif);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(notif.window) {
            return screen.on_destroy_notify(notif);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_configure_notify(&mut self, _notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        Ok(HandleResult::Ignored)
    }

    fn on_randr_notify(&mut self, notif: randr::NotifyEvent) -> Result<HandleResult> {
        match notif.sub_code {
            randr::Notify::CRTC_CHANGE => {
                debug!("CRTC_CHANGE: {:?}", notif.u.as_cc());
                self.setup_monitor()?;
                self.refresh_layout()?;
            }

            randr::Notify::OUTPUT_CHANGE => {
                debug!("OUTPUT_CHANGE: {:?}", notif.u.as_oc());
                self.setup_monitor()?;
                self.refresh_layout()?;
            }
            _ => {}
        }
        Ok(HandleResult::Consumed)
    }
}
