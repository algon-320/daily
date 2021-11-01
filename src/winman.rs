use log::{debug, info, warn};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::screen::Screen;
use crate::window::{Window, WindowState};
use crate::{Command, KeybindAction};

use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::{Window as Wid, *},
    xtest::ConnectionExt as _,
};

fn get_mut_pair<T>(slice: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    assert!(a != b && a < slice.len() && b < slice.len());

    use std::cmp::{max, min};
    let (a, b, swapped) = (min(a, b), max(a, b), a > b);

    // <--------xyz-------->
    // <--x--><-----yz----->
    //        <--y--><--z-->
    // .......a......b......
    let xyz = slice;
    let (_x, yz) = xyz.split_at_mut(a);
    let (y, z) = yz.split_at_mut(b - a);
    let a = &mut y[0];
    let b = &mut z[0];

    if swapped {
        (b, a)
    } else {
        (a, b)
    }
}

const MAX_SCREENS: usize = 10;

#[derive(Debug)]
pub struct Monitor {
    pub id: usize,
    pub info: randr::MonitorInfo,
}

#[derive()]
pub struct WinMan {
    ctx: Context,
    screens: Vec<Screen>,
    monitor_num: usize,
}

impl WinMan {
    pub fn new(ctx: Context) -> Result<Self> {
        let mut wm = Self {
            ctx,
            screens: Vec::new(),
            monitor_num: 0,
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

        let first = self.screens.get_mut(0).expect("no screen");

        // Put all pre-existing windows on the first screen.
        for &wid in preexist.iter() {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;
            if attr.class == WindowClass::INPUT_ONLY {
                continue;
            }

            let state = if attr.map_state == MapState::VIEWABLE {
                WindowState::Mapped
            } else {
                WindowState::Unmapped
            };

            let win = Window::new(self.ctx.clone(), wid, state)?;
            first.add_window(win)?;
        }

        // Focus the first monitor
        first.focus_any()?;

        debug!("screen 0: {:#?}", self.screens[0]);

        self.refresh_layout()?;

        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        self.monitor_num = monitors_reply.monitors.len();

        for screen in self.screens.iter_mut() {
            screen.detach()?;
        }

        for id in self.screens.len()..std::cmp::max(self.monitor_num, MAX_SCREENS) {
            let screen = Screen::new(self.ctx.clone(), id)?;
            self.screens.push(screen);
        }

        for (id, info) in monitors_reply.monitors.into_iter().enumerate() {
            let screen = self.screens.get_mut(id).unwrap();
            screen.attach(Monitor { id, info })?;
        }
        Ok(())
    }

    fn refresh_layout(&mut self) -> Result<()> {
        for screen in self.screens.iter_mut() {
            screen.refresh_layout()?;
        }
        Ok(())
    }

    fn focused_screen_mut(&mut self) -> Result<&mut Screen> {
        let mut id = None;
        if let Some(wid) = self.ctx.get_focused_window()? {
            id = self.container_of_mut(wid).map(|sc| sc.id);
        };
        Ok(self.screens.get_mut(id.unwrap_or(0)).unwrap())
    }

    fn find_screen_mut<P>(&mut self, pred: P) -> Option<&mut Screen>
    where
        P: Fn(&Screen) -> bool,
    {
        for screen in self.screens.iter_mut() {
            if pred(screen) {
                return Some(screen);
            }
        }
        None
    }

    fn container_of_mut(&mut self, wid: Wid) -> Option<&mut Screen> {
        self.find_screen_mut(|screen| screen.contains(wid))
    }

    fn window_mut(&mut self, wid: Wid) -> Option<&mut Window> {
        let screen = self.container_of_mut(wid)?;
        screen.window_mut(wid)
    }

    fn focus_changed(&mut self) -> Result<()> {
        self.refresh_layout()?;
        Ok(())
    }

    fn switch_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        debug!("switch to screen: {}", id);
        if self.screens[id].monitor().is_none() {
            let old = self.focused_screen_mut()?;
            let mon_info = old.detach()?.expect("focus inconsistent");
            let new = self.screens.get_mut(id).unwrap();
            new.attach(mon_info)?;
        } else {
            let a = self.focused_screen_mut()?.id;
            let b = self.screens.iter().position(|sc| sc.id == id).unwrap();

            // no swap needed
            if a == b {
                return Ok(());
            }

            // perfom swap
            let (screen_a, screen_b) = get_mut_pair(&mut self.screens, a, b);
            Screen::swap_monitor(screen_a, screen_b)?;
        }

        let screen = self.screens.get_mut(id).unwrap();
        screen.focus_any()?;
        self.focus_changed()?;

        Ok(())
    }

    fn move_window_to_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        if let Some(wid) = self.ctx.get_focused_window()? {
            let screen = self.focused_screen_mut()?;

            if wid == screen.background() {
                return Ok(());
            }

            debug!("move_window_to_screen: wid = {}", wid);

            let win = screen.forget_window(wid)?;
            screen.focus_any()?;

            let screen = self.screens.get_mut(id).unwrap();
            screen.add_window(win)?;

            self.focus_changed()?;
        }

        Ok(())
    }

    fn move_pointer(&mut self, dx: i16, dy: i16) -> Result<()> {
        self.ctx
            .conn
            .warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, dx, dy)?;
        Ok(())
    }

    fn simulate_click(&mut self, button: u8, duration_ms: u32) -> Result<()> {
        // button down
        self.ctx.conn.xtest_fake_input(
            BUTTON_PRESS_EVENT,
            button,
            x11rb::CURRENT_TIME,
            x11rb::NONE,
            0,
            0,
            0,
        )?;

        // button up
        self.ctx.conn.xtest_fake_input(
            BUTTON_RELEASE_EVENT,
            button,
            duration_ms,
            x11rb::NONE,
            0,
            0,
            0,
        )?;

        Ok(())
    }

    fn spawn_process(&self, cmd: &str) -> Result<()> {
        let _ = std::process::Command::new("sh").arg("-c").arg(cmd).spawn();
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),

            Command::ShowBorder => {
                for screen in self.screens.iter_mut() {
                    screen.show_border();
                }
                self.refresh_layout()?;
            }
            Command::HideBorder => {
                for screen in self.screens.iter_mut() {
                    screen.hide_border();
                }
                self.refresh_layout()?;
            }

            Command::Close => {
                if let Some(focused) = self.ctx.get_focused_window()? {
                    let wid = self.window_mut(focused).map(|w| w.id());
                    if let Some(wid) = wid {
                        self.ctx.conn.destroy_window(wid)?;
                    }
                }
            }

            Command::FocusNext => {
                self.focused_screen_mut()?.focus_next()?;
                self.focus_changed()?;
            }
            Command::FocusPrev => {
                warn!("Command::FocusPrev: not yet implemented");
            }

            Command::FocusNextMonitor => {
                let focused_monitor = self
                    .focused_screen_mut()?
                    .monitor()
                    .expect("focus inconsistent")
                    .id;
                let next_monitor = (focused_monitor + 1) % self.monitor_num;

                let screen = self
                    .find_screen_mut(|screen| {
                        screen.monitor().map(|mon| mon.id) == Some(next_monitor)
                    })
                    .unwrap();
                screen.focus_any()?;

                self.focus_changed()?;
            }
            Command::FocusPrevMonitor => {
                let focused_monitor = self
                    .focused_screen_mut()?
                    .monitor()
                    .expect("focus inconsistent")
                    .id;
                let next_monitor = (focused_monitor + self.monitor_num - 1) % self.monitor_num;

                let screen = self
                    .find_screen_mut(|screen| {
                        screen.monitor().map(|mon| mon.id) == Some(next_monitor)
                    })
                    .unwrap();
                screen.focus_any()?;

                self.focus_changed()?;
            }

            Command::NextLayout => {
                let screen = self.focused_screen_mut()?;
                screen.next_layout()?;
            }

            Command::Spawn(cmd) => self.spawn_process(&cmd)?,

            Command::Screen(id) => self.switch_screen(id)?,
            Command::MoveToScreen(id) => self.move_window_to_screen(id)?,
            Command::MovePointerRel(dx, dy) => self.move_pointer(dx, dy)?,
            Command::MouseClickLeft => self.simulate_click(1, 10)?, // left, 10ms
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

        // TODO: background
        let win = self.window_mut(e.child);
        if e.child != x11rb::NONE && win.is_some() {
            win.unwrap().focus()?;
            self.focus_changed()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let attr = self.ctx.conn.get_window_attributes(notif.window)?.reply()?;
            if attr.class == WindowClass::INPUT_ONLY {
                return Ok(HandleResult::Consumed);
            }

            if self.container_of_mut(notif.window).is_some() {
                return Ok(HandleResult::Ignored);
            }

            let win = Window::new(self.ctx.clone(), notif.window, WindowState::Created)?;

            let screen = self.focused_screen_mut()?;
            screen.add_window(win)?;

            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(req.window) {
            screen.on_map_request(req)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let wid = notif.window;
            if let Some(screen) = self.container_of_mut(wid) {
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
            let res = screen.on_destroy_notify(notif);
            self.focus_changed()?;
            return res;
        }
        Ok(HandleResult::Ignored)
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(req.window) {
            screen.on_configure_request(req)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            if let Some(screen) = self.container_of_mut(notif.window) {
                return screen.on_configure_notify(notif);
            }
        }
        Ok(HandleResult::Ignored)
    }

    fn on_focus_in(&mut self, focus_in: FocusInEvent) -> Result<HandleResult> {
        if focus_in.event == self.ctx.root {
            if focus_in.detail == NotifyDetail::POINTER_ROOT {
                self.focused_screen_mut()?.focus_any()?;
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
