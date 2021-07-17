use log::{debug, error, info, trace, warn};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::{Command, KeybindAction};

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _, MonitorInfo},
    xproto::*,
    ErrorKind,
};
use x11rb::rust_connection::RustConnection;
use x11rb::x11_utils::X11Error;
use Window as Wid;

#[derive(Debug, Clone)]
struct Context {
    conn: Rc<RustConnection>,
    config: Rc<Config>,
    root: Wid,
}

#[derive(Debug)]
struct Screen {
    ctx: Context,
    u_wins: HashSet<Wid>,
    m_wins: HashSet<Wid>,
    monitor: Option<MonitorInfo>,
    layout: HorizontalLayout,
}

impl Screen {
    fn new_(ctx: Context, monitor: Option<MonitorInfo>) -> Self {
        let layout = HorizontalLayout::new(ctx.clone());
        Self {
            ctx,
            u_wins: HashSet::new(),
            m_wins: HashSet::new(),
            monitor,
            layout,
        }
    }

    pub fn new(ctx: Context) -> Self {
        Self::new_(ctx, None)
    }

    pub fn with_monitor(ctx: Context, monitor: MonitorInfo) -> Self {
        Self::new_(ctx, Some(monitor))
    }

    pub fn show(&mut self, monitor: MonitorInfo) {
        self.monitor = Some(monitor);
    }
    pub fn hide(&mut self) {
        self.monitor = None;
    }

    pub fn add_window(&mut self, wid: Wid, mapped: bool) {
        if mapped {
            self.m_wins.insert(wid);
        } else {
            self.u_wins.insert(wid);
        }
    }

    pub fn update_layout(&mut self) -> Result<()> {
        // FIXME
        let wins: Vec<Wid> = self.m_wins.iter().copied().collect();

        let mon = self.monitor.as_ref().expect("screen not visible");
        self.layout.layout(mon, &wins)?;
        Ok(())
    }
}

impl EventHandlerMethods for Screen {
    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.u_wins.contains(&wid) {
            self.u_wins.remove(&wid);
            self.m_wins.insert(wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.m_wins.contains(&wid) {
            self.m_wins.remove(&wid);
            self.u_wins.insert(wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.m_wins.contains(&wid) {
            self.m_wins.remove(&wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else if self.u_wins.contains(&wid) {
            self.u_wins.remove(&wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }
}

#[derive(Debug)]
struct HorizontalLayout {
    ctx: Context,
}

impl HorizontalLayout {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    pub fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid]) -> Result<()> {
        if windows.is_empty() {
            return Ok(());
        }

        let count = windows.len();
        let w = (mon.width / count as u16) as u32;
        let h = mon.height as u32;
        let offset_x = mon.x as i32;
        let offset_y = mon.y as i32;
        let mut x = 0;

        for &wid in windows.iter() {
            let border = self.ctx.config.border.clone();
            let conf = ConfigureWindowAux::new()
                .x(offset_x + x)
                .y(offset_y)
                .border_width(border.width)
                .width(w - border.width * 2)
                .height(h - border.width * 2);
            self.ctx.conn.configure_window(wid, &conf)?;
            x += w as i32;
        }
        self.ctx.conn.flush()?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct WinMan {
    ctx: Context,
    monitors: Vec<randr::MonitorInfo>,
    screens: Vec<Screen>,
}

impl WinMan {
    pub fn new(conn: Rc<RustConnection>, config: Rc<Config>, root: Wid) -> Result<Self> {
        let mut wm = Self {
            ctx: Context { conn, config, root },
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
        self.ctx
            .conn
            .randr_select_input(self.ctx.root, randr::NotifyMask::OUTPUT_CHANGE)?;

        // Setup monitors
        self.setup_monitor()?;
        if self.monitors.is_empty() {
            return Err(Error::NoMonitor);
        }

        // Setup virtual screens
        for mon in self.monitors.iter() {
            self.screens
                .push(Screen::with_monitor(self.ctx.clone(), mon.clone()));
        }

        // Put all pre-existing windows on the first virtual screen.
        let preexist = self.ctx.conn.query_tree(self.ctx.root)?.reply()?.children;
        info!("preexist windows = {:?}", &preexist);
        for wid in preexist {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;
            let mapped = attr.map_state == MapState::VIEWABLE;
            let first = self.screens.get_mut(0).expect("no screen");
            first.add_window(wid, mapped);
        }

        // Refresh layouts
        for screen in self.screens.iter_mut() {
            screen.update_layout()?;
        }

        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        self.monitors = monitors_reply.monitors;
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),

            Command::ShowBorder => {
                let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let color = self.ctx.config.border.color_focused;
                    let attr = ChangeWindowAttributesAux::new().border_pixel(color);
                    self.ctx.conn.change_window_attributes(focused, &attr)?;
                    self.ctx.conn.flush()?;
                }
            }
            Command::HideBorder => {
                let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
                if focused != InputFocus::POINTER_ROOT.into() {
                    let color = self.ctx.config.border.color_regular;
                    let attr = ChangeWindowAttributesAux::new().border_pixel(color);
                    self.ctx.conn.change_window_attributes(focused, &attr)?;
                    self.ctx.conn.flush()?;
                }
            }

            Command::Close => {
                let focused = self.ctx.conn.get_input_focus()?.reply()?.focus;
                self.ctx.conn.destroy_window(focused)?;
                self.ctx.conn.flush()?;
            }

            Command::FocusNext => {
                warn!("Command::FocusNext: not yet implemented");
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

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        self.ctx.conn.map_window(req.window)?;
        self.ctx.conn.flush()?;
        Ok(HandleResult::Consumed)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            for screen in self.screens.iter_mut() {
                match screen.on_map_notify(notif) {
                    Ok(HandleResult::Ignored) => continue,
                    otherwise => return otherwise,
                }
            }
        }
        Ok(HandleResult::Ignored)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        for screen in self.screens.iter_mut() {
            match screen.on_unmap_notify(notif) {
                Ok(HandleResult::Ignored) => continue,
                otherwise => return otherwise,
            }
        }
        Ok(HandleResult::Ignored)
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
            for sc in self.screens.iter_mut() {
                if let Some(mon) = sc.monitor.as_mut() {
                    if mon.x <= x
                        && x < mon.x + mon.width as i16
                        && mon.y <= y
                        && y < mon.y + mon.height as i16
                    {
                        info!("pointer on {:?}", mon);
                        screen = Some(sc);
                        break;
                    }
                }
            }
            let screen = match screen {
                Some(sc) => sc,
                None => self.screens.get_mut(0).expect("no screen"),
            };
            screen.add_window(notif.window, false);

            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        for screen in self.screens.iter_mut() {
            match screen.on_destroy_notify(notif) {
                Ok(HandleResult::Ignored) => continue,
                otherwise => return otherwise,
            }
        }
        Ok(HandleResult::Ignored)
    }
}
