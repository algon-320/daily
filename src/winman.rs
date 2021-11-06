use log::{debug, error, info, warn};

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::{Window as Wid, *},
    xtest::ConnectionExt as _,
};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::screen::Screen;
use crate::window::{Window, WindowState};
use crate::{Command, KeybindAction};

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

fn spawn_process(cmd: &str) -> Result<()> {
    let _ = std::process::Command::new("sh").arg("-c").arg(cmd).spawn();
    Ok(())
}

fn move_pointer<C: Connection>(conn: &C, dx: i16, dy: i16) -> Result<()> {
    conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, dx, dy)?;
    Ok(())
}

fn simulate_click<C: Connection>(conn: &C, button: u8, duration_ms: u32) -> Result<()> {
    // button down
    conn.xtest_fake_input(
        BUTTON_PRESS_EVENT,
        button,
        x11rb::CURRENT_TIME,
        x11rb::NONE,
        0,
        0,
        0,
    )?;

    // button up
    conn.xtest_fake_input(
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

        let first = &mut self.screens[0];

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

        for (id, screen) in self.screens.iter().enumerate() {
            debug!("[{}]: screen {}: {:#?}", id, screen.id, screen);
        }

        self.refresh_layout()?;

        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        self.ctx.focus_window(self.ctx.root)?; // HACK

        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        self.monitor_num = monitors_reply.monitors.len();

        for screen in self.screens.iter_mut() {
            let _ = screen.detach()?;
        }

        let max_num = std::cmp::max(self.monitor_num, MAX_SCREENS);
        while self.screens.len() < max_num {
            let id = self.screens.len();
            let screen = Screen::new(self.ctx.clone(), id)?;
            self.screens.push(screen);
        }

        for (id, info) in monitors_reply.monitors.into_iter().enumerate() {
            let screen = &mut self.screens[id];
            screen.attach(Monitor { id, info })?;
        }
        Ok(())
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

    fn screen_mut_by_mon(&mut self, mon_id: usize) -> &mut Screen {
        self.find_screen_mut(|screen| {
            if let Some(mon) = screen.monitor() {
                mon.id == mon_id
            } else {
                false
            }
        })
        .expect("Monitor lost")
    }

    fn focused_screen_mut(&mut self) -> Result<&mut Screen> {
        let mut id = None;
        if let Some(wid) = self.ctx.get_focused_window()? {
            id = self.container_of_mut(wid).map(|sc| sc.id);
        };
        let id = id.unwrap_or_else(|| self.screen_mut_by_mon(0).id);
        Ok(&mut self.screens[id])
    }

    fn refresh_layout(&mut self) -> Result<()> {
        for screen in self.screens.iter_mut() {
            screen.refresh_layout()?;
        }
        Ok(())
    }

    fn focus_changed(&mut self) -> Result<()> {
        self.refresh_layout()?;
        Ok(())
    }

    fn switch_screen(&mut self, id: usize) -> Result<()> {
        if id >= self.screens.len() {
            error!("winman.switch_screen: invalid id = {}", id);
            return Ok(());
        }

        debug!("switch to screen: {}", id);

        if self.screens[id].monitor().is_none() {
            let old_id = self.focused_screen_mut()?.id;

            // HACK:
            //   Avoid generation of FocusIn event with detail=PointerRoot/None
            //   between a detach and the following attach.
            self.ctx.focus_window(self.ctx.root)?;

            let old = &mut self.screens[old_id];
            let mon_info = old.detach()?.expect("focus inconsistent");

            let new = &mut self.screens[id];
            new.attach(mon_info)?;
            new.focus_any()?;
        } else {
            let a = self.focused_screen_mut()?.id;
            let b = id;

            // no swap needed
            if a == b {
                return Ok(());
            }

            // HACK:
            //   Avoid generation of FocusIn event with detail=PointerRoot/None
            //   between a detach and the following attach.
            self.ctx.focus_window(self.ctx.root)?;

            // perfom swap
            let (screen_a, screen_b) = get_mut_pair(&mut self.screens, a, b);
            let mon_a = screen_a.detach()?.expect("focus inconsistent");
            let mon_b = screen_b.detach()?.expect("focus inconsistent");

            screen_a.attach(mon_b)?;
            screen_b.attach(mon_a)?;

            screen_b.focus_any()?;
        }

        self.focus_changed()?;
        Ok(())
    }

    fn move_window_to_screen(&mut self, id: usize) -> Result<()> {
        if id >= self.screens.len() {
            error!("winman.switch_screen: invalid id = {}", id);
            return Ok(());
        }

        let focus = self.ctx.get_focused_window()?;
        if let Some(wid) = focus {
            if let Some(src) = self.container_of_mut(wid) {
                if src.background().contains(wid) || src.bar().contains(wid) {
                    return Ok(());
                }

                debug!("move_window_to_screen: wid = {}", wid);

                let win = src.forget_window(wid)?;
                src.focus_any()?;

                let dst = &mut self.screens[id];
                dst.add_window(win)?;

                self.focus_changed()?;
            }
        }
        Ok(())
    }

    fn focus_monitor(&mut self, mon_id: usize) -> Result<()> {
        let screen = self.screen_mut_by_mon(mon_id);
        screen.focus_any()?;
        self.focus_changed()?;
        Ok(())
    }

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Quit => return Err(Error::Quit),
            Command::Restart => return Err(Error::Restart),

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
                if let Some(wid) = self.ctx.get_focused_window()? {
                    if let Some(screen) = self.container_of_mut(wid) {
                        if !screen.background().contains(wid) && !screen.bar().contains(wid) {
                            let _ = screen.forget_window(wid);
                            self.refresh_layout()?;
                        }
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
                self.focus_monitor(next_monitor)?;
            }
            Command::FocusPrevMonitor => {
                let focused_monitor = self
                    .focused_screen_mut()?
                    .monitor()
                    .expect("focus inconsistent")
                    .id;
                let prev_monitor = (focused_monitor + self.monitor_num - 1) % self.monitor_num;
                self.focus_monitor(prev_monitor)?;
            }

            Command::NextLayout => {
                let screen = self.focused_screen_mut()?;
                screen.next_layout()?;
            }

            Command::Screen(id) => self.switch_screen(id)?,
            Command::MoveToScreen(id) => self.move_window_to_screen(id)?,

            Command::MovePointerRel(dx, dy) => move_pointer(&self.ctx.conn, dx, dy)?,
            Command::MouseClickLeft => simulate_click(&self.ctx.conn, 1, 10)?, // left, 10ms
            Command::Spawn(cmd) => spawn_process(&cmd)?,
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
        if let Some(win) = self.window_mut(req.window) {
            let res = win.on_map_request(req);
            self.focus_changed()?;
            return res;
        }
        Ok(HandleResult::Ignored)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let wid = notif.window;
            if let Some(win) = self.window_mut(wid) {
                let res = win.on_map_notify(notif);
                self.focus_changed()?;
                return res;
            }
        }
        Ok(HandleResult::Ignored)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if let Some(win) = self.window_mut(notif.window) {
            let res = win.on_unmap_notify(notif);
            self.focus_changed()?;
            return res;
        }
        Ok(HandleResult::Ignored)
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(notif.window) {
            let _ = screen.forget_window(notif.window)?;
            self.focus_changed()?;
            return Ok(HandleResult::Consumed);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<HandleResult> {
        if let Some(win) = self.window_mut(req.window) {
            return win.on_configure_request(req);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            if let Some(win) = self.window_mut(notif.window) {
                return win.on_configure_notify(notif);
            }
        }
        Ok(HandleResult::Ignored)
    }

    fn on_expose(&mut self, ev: ExposeEvent) -> Result<HandleResult> {
        if let Some(screen) = self.container_of_mut(ev.window) {
            return screen.on_expose(ev);
        }
        Ok(HandleResult::Ignored)
    }

    fn on_focus_in(&mut self, focus_in: FocusInEvent) -> Result<HandleResult> {
        if focus_in.event == self.ctx.root {
            if focus_in.detail == NotifyDetail::POINTER_ROOT
                || focus_in.detail == NotifyDetail::NONE
            {
                // Focus the first monitor
                let screen = self.screen_mut_by_mon(0);
                screen.focus_any()?;
            }
            return Ok(HandleResult::Consumed);
        }

        Ok(HandleResult::Ignored)
    }

    fn on_randr_notify(&mut self, notif: randr::NotifyEvent) -> Result<HandleResult> {
        match notif.sub_code {
            randr::Notify::CRTC_CHANGE => {
                debug!("CRTC_CHANGE: {:?}", notif.u.as_cc());
                self.setup_monitor()?;
                self.screens[0].focus_any()?;
                self.focus_changed()?;
            }

            randr::Notify::OUTPUT_CHANGE => {
                debug!("OUTPUT_CHANGE: {:?}", notif.u.as_oc());
                self.setup_monitor()?;
                self.screens[0].focus_any()?;
                self.focus_changed()?;
            }
            _ => {}
        }
        Ok(HandleResult::Consumed)
    }
}
