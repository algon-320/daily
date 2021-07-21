use log::{debug, error, info, trace, warn};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::keybind::{Command, KeybindAction};
use crate::screen::Screen;

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
};
use Window as Wid;

#[derive(Debug)]
struct Monitor {
    info: randr::MonitorInfo,
    screen: usize,
}

#[derive()]
pub struct WinMan {
    ctx: Context,
    monitors: Vec<Monitor>,
    screens: Vec<Screen>,
}

impl WinMan {
    pub fn new(ctx: Context) -> Result<Self> {
        let mut wm = Self {
            ctx,
            monitors: Vec::new(),
            screens: Vec::new(),
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
        self.ctx.conn.randr_select_input(
            self.ctx.root,
            randr::NotifyMask::OUTPUT_CHANGE | randr::NotifyMask::CRTC_CHANGE,
        )?;

        // Setup monitors
        self.setup_monitor()?;

        // Put all pre-existing windows on the first virtual screen.
        let preexist = self.ctx.conn.query_tree(self.ctx.root)?.reply()?.children;
        info!("preexist windows = {:?}", &preexist);
        for wid in preexist {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;
            let mapped = attr.map_state == MapState::VIEWABLE;
            let first = self.screens.get_mut(0).expect("no screen");
            first.add_window(wid, mapped);
        }

        self.refresh_layout()?;

        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        self.monitors = monitors_reply
            .monitors
            .into_iter()
            .enumerate()
            .map(|(idx, info)| Monitor { info, screen: idx })
            .collect();

        if self.monitors.is_empty() {
            return Err(Error::NoMonitor);
        }

        if self.monitors.len() > self.screens.len() {
            // Setup virtual screens
            for mon in self.monitors.iter().skip(self.screens.len()) {
                self.screens
                    .push(Screen::with_monitor(self.ctx.clone(), mon.info.clone()));
            }
        }

        for mon in self.monitors.iter() {
            self.screens.get_mut(mon.screen).unwrap().monitor = Some(mon.info.clone());
        }
        Ok(())
    }

    fn refresh_layout(&mut self) -> Result<()> {
        for screen in self.screens.iter_mut() {
            screen.update_layout()?;
        }
        Ok(())
    }

    fn change_border_color(&self, wid: Wid, color: u32) -> Result<()> {
        let attr = ChangeWindowAttributesAux::new().border_pixel(color);
        self.ctx.conn.change_window_attributes(wid, &attr)?;
        self.ctx.conn.flush()?;
        Ok(())
    }

    fn container_of_mut(&mut self, wid: Window) -> Option<&'_ mut Screen> {
        for screen in self.screens.iter_mut() {
            if screen.contains(wid).is_some() {
                return Some(screen);
            }
        }
        None
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),

            Command::ShowBorder => {
                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let color = self.ctx.config.border.color_focused;
                    self.change_border_color(focused, color)?;
                }
            }
            Command::HideBorder => {
                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let color = self.ctx.config.border.color_regular;
                    self.change_border_color(focused, color)?;
                }
            }

            Command::Close => {
                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    self.ctx.conn.destroy_window(focused)?;
                    self.ctx.conn.flush()?;
                }
            }

            Command::FocusNext => {
                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let cr = self.ctx.config.border.color_regular;
                    self.change_border_color(focused, cr)?;
                }

                let screen = match self.container_of_mut(focused) {
                    Some(sc) => sc,
                    None => self.screens.get_mut(0).expect("no screen"),
                };

                screen.focus_next()?;

                let focused = self.ctx.get_focused_window()?;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let cf = self.ctx.config.border.color_focused;
                    self.change_border_color(focused, cf)?;
                }
            }
            Command::FocusPrev => {
                warn!("Command::FocusPrev: not yet implemented");
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

    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let color = self.ctx.config.border.color_regular;
            let attr = ChangeWindowAttributesAux::new().border_pixel(color);
            self.ctx
                .conn
                .change_window_attributes(notif.window, &attr)?;
            self.ctx.conn.flush()?;

            // FIXME:
            let pointer = self.ctx.conn.query_pointer(self.ctx.root)?.reply()?;
            let x = pointer.root_x;
            let y = pointer.root_y;
            let mut screen = None;
            for mon in self.monitors.iter() {
                let info = &mon.info;
                if info.x <= x
                    && x < info.x + info.width as i16
                    && info.y <= y
                    && y < info.y + info.height as i16
                {
                    info!("pointer on {:?}", mon);
                    screen = Some(mon.screen);
                    break;
                }
            }
            self.screens
                .get_mut(screen.unwrap_or(0))
                .expect("invalid screen")
                .add_window(notif.window, false);

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
            if let Some(screen) = self.container_of_mut(notif.window) {
                return screen.on_map_notify(notif);
            }
            warn!("orphan window: {}", notif.window);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(notif.window) {
            return screen.on_unmap_notify(notif);
        }
        warn!("orphan window: {}", notif.window);
        Ok(HandleResult::Ignored)
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(notif.window) {
            return screen.on_destroy_notify(notif);
        }
        warn!("orphan window: {}", notif.window);
        Ok(HandleResult::Ignored)
    }

    fn on_randr_notify(&mut self, notif: randr::NotifyEvent) -> Result<HandleResult> {
        trace!("on_randr_notify: {:?}", notif);

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
