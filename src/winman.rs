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

const MAX_SCREENS: usize = 10;

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
        let mask = EventMask::SUBSTRUCTURE_NOTIFY
            | EventMask::SUBSTRUCTURE_REDIRECT
            | EventMask::FOCUS_CHANGE;
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

            first.add_window(wid, state)?;
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
            let mut screen = mon.screen;
            screen.detach()?;
            self.screens.insert(screen.id, screen);
        }

        for id in 0..MAX_SCREENS {
            if let HashMapEntry::Vacant(entry) = self.screens.entry(id) {
                let screen = Screen::new(self.ctx.clone(), id)?;
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

        Ok(())
    }

    fn refresh_layout(&mut self) -> Result<()> {
        for mon in self.monitors.iter_mut() {
            mon.screen.refresh_layout()?;
        }
        Ok(())
    }

    fn focused_monitor_mut(&mut self) -> &mut Monitor {
        self.monitors.get_mut(self.focused_monitor).unwrap()
    }

    fn find_screen_mut<P>(&mut self, pred: P) -> Option<&mut Screen>
    where
        P: Fn(&Screen) -> bool,
    {
        for mon in self.monitors.iter_mut() {
            if pred(&mon.screen) {
                return Some(&mut mon.screen);
            }
        }

        for screen in self.screens.values_mut() {
            if pred(&screen) {
                return Some(screen);
            }
        }

        None
    }

    fn container_of_mut(&mut self, wid: Wid) -> Option<&mut Screen> {
        self.find_screen_mut(|screen| screen.contains(wid).is_some())
    }

    fn change_focus(&mut self, wid: Wid) -> Result<()> {
        debug!("change_focus: wid={}", wid);
        self.ctx.focus_window(wid)?;
        self.focus_changed()?;
        Ok(())
    }

    fn focus_changed(&mut self) -> Result<()> {
        let focused_win = self.ctx.get_focused_window()?;
        debug!("focused_win = {}", focused_win);
        if focused_win != InputFocus::POINTER_ROOT.into() {
            self.focused_monitor = self
                .monitors
                .iter()
                .position(|mon| mon.screen.contains(focused_win).is_some())
                .unwrap();
        }
        self.refresh_layout()?;
        Ok(())
    }

    fn switch_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        debug!("switch to screen: {}", id);
        if self.screens.contains_key(&id) {
            let mon = self.focused_monitor_mut();
            mon.screen.detach()?;

            let new = self.screens.remove(&id).unwrap();

            let mon = self.focused_monitor_mut();
            let old = std::mem::replace(&mut mon.screen, new);
            mon.screen.attach(mon.info.clone())?;

            self.screens.insert(old.id, old);
        } else {
            let a = self.focused_monitor;
            let b = self
                .monitors
                .iter()
                .position(|mon| mon.screen.id == id)
                .unwrap();

            // no swap needed
            if a == b {
                return Ok(());
            }

            // perfom swap
            {
                use std::cmp::{max, min};
                let (a, b) = (min(a, b), max(a, b));

                let (_, mons) = self.monitors.split_at_mut(a);
                let (mon_a, mon_b) = mons.split_at_mut(b - a);
                let mon_a = &mut mon_a[0];
                let mon_b = &mut mon_b[0];

                mon_a.screen.detach()?;
                mon_b.screen.detach()?;

                std::mem::swap(&mut mon_a.screen, &mut mon_b.screen);

                mon_a.screen.attach(mon_a.info.clone())?;
                mon_b.screen.attach(mon_b.info.clone())?;
            }
        }

        let mon = self.focused_monitor_mut();
        mon.screen.focus_any()?;
        self.focus_changed()?;
        Ok(())
    }

    fn move_window_to_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        let wid = self.ctx.get_focused_window()?;
        debug!("move_window_to_screen: wid = {}", wid);

        if wid != InputFocus::POINTER_ROOT.into() {
            let mon = self.focused_monitor_mut();

            let state = mon.screen.forget_window(wid)?;
            mon.screen.focus_any()?;
            self.focus_changed()?;

            if let Some(screen) = self.find_screen_mut(|screen| screen.id == id) {
                screen.add_window(wid, state)?;
                return Ok(());
            }
        }

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
                let mon = self.focused_monitor_mut();
                mon.screen.focus_next()?;
                self.focus_changed()?;
            }
            Command::FocusPrev => {
                warn!("Command::FocusPrev: not yet implemented");
            }

            Command::FocusNextMonitor => {
                // FIXME
                self.focused_monitor = (self.focused_monitor + 1) % self.monitors.len();
                let next_mon = self.focused_monitor_mut();
                next_mon.screen.focus_any()?;
                self.focus_changed()?;
            }
            Command::FocusPrevMonitor => {
                warn!("Command::FocusPrevMonitor: not yet implemented");
            }

            Command::OpenLauncher => {
                let _ = std::process::Command::new(self.ctx.config.launcher.as_str()).spawn();
            }

            Command::Screen1 => self.switch_screen(0)?,
            Command::Screen2 => self.switch_screen(1)?,
            Command::Screen3 => self.switch_screen(2)?,
            Command::Screen4 => self.switch_screen(3)?,
            Command::Screen5 => self.switch_screen(4)?,

            Command::MoveToScreen1 => self.move_window_to_screen(0)?,
            Command::MoveToScreen2 => self.move_window_to_screen(1)?,
            Command::MoveToScreen3 => self.move_window_to_screen(2)?,
            Command::MoveToScreen4 => self.move_window_to_screen(3)?,
            Command::MoveToScreen5 => self.move_window_to_screen(4)?,
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
            let mon = self.focused_monitor_mut();
            mon.screen.add_window(notif.window, WindowState::Created)?;

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
                let res = screen.on_map_notify(notif);
                self.focus_changed()?;
                return res;
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

    fn on_focus_in(&mut self, focus_in: FocusInEvent) -> Result<HandleResult> {
        if focus_in.event == self.ctx.root {
            if focus_in.detail == NotifyDetail::POINTER_ROOT {
                let mon = self.focused_monitor_mut();
                mon.screen.focus_any()?;
            }
            return Ok(HandleResult::Consumed);
        }

        if let Some(screen) = self.container_of_mut(focus_in.event) {
            return screen.on_focus_in(focus_in);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_focus_out(&mut self, focus_out: FocusInEvent) -> Result<HandleResult> {
        if focus_out.event == self.ctx.root {
            return Ok(HandleResult::Consumed);
        }

        if let Some(screen) = self.container_of_mut(focus_out.event) {
            return screen.on_focus_out(focus_out);
        }
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
