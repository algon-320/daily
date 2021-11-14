use log::{debug, error, info, warn};

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::{Window as Wid, *},
    xtest::ConnectionExt as _,
};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::EventHandlerMethods;
use crate::monitor::Monitor;
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
    use std::process::{Command, Stdio};
    let mut cmd = cmd.to_owned();
    cmd.push_str(" &");
    if let Ok(mut sh) = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        let _ = sh.wait();
    }
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

#[derive(Debug, Clone)]
struct MouseDrag {
    wid: Wid,
    state: u16,
    start_x: i16,
    start_y: i16,
    window_x: i16,
    window_y: i16,
    window_w: u16,
    window_h: u16,
}

#[derive()]
pub struct WinMan {
    ctx: Context,
    screens: Vec<Screen>,
    monitor_num: usize,
    drag: Option<MouseDrag>,
}

impl WinMan {
    pub fn new(ctx: Context) -> Result<Self> {
        let mut wm = Self {
            ctx,
            screens: Vec::new(),
            monitor_num: 0,
            drag: None,
        };
        wm.init()?;
        Ok(wm)
    }

    fn init(&mut self) -> Result<()> {
        // Become a window manager of the root window.
        let mask = EventMask::SUBSTRUCTURE_NOTIFY
            | EventMask::SUBSTRUCTURE_REDIRECT
            | EventMask::FOCUS_CHANGE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::BUTTON_MOTION;
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
        let event_mask: u32 =
            (EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION).into();
        // Mouse left and right button
        for button in [ButtonIndex::M1, ButtonIndex::M3] {
            self.ctx
                .conn
                .grab_button(
                    false,
                    self.ctx.root,
                    event_mask as u16,
                    GrabMode::SYNC,  // pointer
                    GrabMode::ASYNC, // keyboard
                    self.ctx.root,
                    x11rb::NONE,
                    button,
                    ModMask::ANY,
                )?
                .check()
                .map_err(|_| Error::ButtonAlreadyGrabbed)?;
        }

        // Receive RROutputChangeNotify / RRCrtcChangeNotify
        self.ctx.conn.randr_select_input(
            self.ctx.root,
            randr::NotifyMask::OUTPUT_CHANGE | randr::NotifyMask::CRTC_CHANGE,
        )?;

        // Setup screens and attach monitors
        self.setup_monitor()?;

        // Put all pre-existing windows on the first screen.
        let preexist = self.ctx.conn.query_tree(self.ctx.root)?.reply()?.children;
        info!("preexist windows = {:08X?}", &preexist);
        let first = &mut self.screens[0];
        for &wid in preexist.iter() {
            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;

            // Ignore uninteresting windows
            if attr.override_redirect || attr.class == WindowClass::INPUT_ONLY {
                continue;
            }

            let state = if attr.map_state == MapState::VIEWABLE {
                WindowState::Mapped
            } else {
                WindowState::Unmapped
            };

            let border_width = self.ctx.config.border.width;
            let win = Window::new(self.ctx.clone(), wid, state, border_width)?;
            first.add_window(win)?;
        }

        // Focus the first monitor
        first.focus_any()?;

        for (id, screen) in self.screens.iter().enumerate() {
            debug!("[{}]: screen {}: {:#?}", id, screen.id, screen);
        }

        self.refresh_layout()?;

        self.ctx.conn.flush()?;
        Ok(())
    }

    fn setup_monitor(&mut self) -> Result<()> {
        self.ctx.focus_window(self.ctx.root)?; // HACK

        // Request monitor info
        let monitors_reply = self
            .ctx
            .conn
            .randr_get_monitors(self.ctx.root, true)?
            .reply()?;
        self.monitor_num = monitors_reply.monitors.len();

        // Detach all monitors
        for screen in self.screens.iter_mut() {
            let _old = screen.detach()?;
        }

        // Fill self.screens
        let max_num = std::cmp::max(self.monitor_num, MAX_SCREENS);
        while self.screens.len() < max_num {
            let id = self.screens.len();
            let screen = Screen::new(self.ctx.clone(), id)?;
            self.screens.push(screen);
        }

        // Attach monitors
        for (id, info) in monitors_reply.monitors.into_iter().enumerate() {
            let new = Monitor::new(&self.ctx, id, info);
            self.screens[id].attach(new)?;
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

        let current_id = self.focused_screen_mut()?.id;

        if self.screens[id].monitor().is_none() {
            // HACK:
            //   Avoid generation of FocusIn event with detail=PointerRoot/None
            //   between a detach and the following attach.
            self.ctx.focus_window(self.ctx.root)?;

            let current = &mut self.screens[current_id];
            let mon_info = current.detach()?.expect("focus inconsistent");

            let new = &mut self.screens[id];
            new.attach(mon_info)?;
            new.focus_any()?;
        } else {
            let a = current_id;
            let b = id;

            // no swap needed
            if a == b {
                return Ok(());
            }

            // perfom swap
            let (screen_a, screen_b) = get_mut_pair(&mut self.screens, a, b);
            Screen::swap_monitors(screen_a, screen_b)?;
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
                if src.background().contains(wid) {
                    return Ok(());
                }

                debug!("move_window_to_screen: wid = {:08X}", wid);

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
                        if !screen.background().contains(wid) {
                            screen.forget_window(wid)?.close();
                            self.refresh_layout()?;
                        }
                    }
                }
            }

            Command::Sink => {
                if let Some(wid) = self.ctx.get_focused_window()? {
                    if let Some(screen) = self.container_of_mut(wid) {
                        if !screen.background().contains(wid) {
                            let win = screen.window_mut(wid).unwrap();
                            win.sink()?;
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

    pub fn alarm(&mut self) -> Result<()> {
        for screen in self.screens.iter_mut() {
            screen.alarm()?;
        }
        Ok(())
    }
}

macro_rules! unwrap_or_return {
    ( $e:expr ) => {
        match $e {
            Some(x) => x,
            None => return Ok(()),
        }
    };
}

impl EventHandlerMethods for WinMan {
    fn on_key_press(&mut self, e: KeyPressEvent) -> Result<()> {
        let cmd = unwrap_or_return!(self.ctx.config.keybind_match(
            KeybindAction::Press,
            e.state,
            e.detail
        ));
        debug!("on_key_press: cmd = {:?}", cmd);
        self.process_command(cmd)?;
        Ok(())
    }

    fn on_key_release(&mut self, e: KeyReleaseEvent) -> Result<()> {
        let cmd = unwrap_or_return!(self.ctx.config.keybind_match(
            KeybindAction::Release,
            e.state,
            e.detail
        ));
        debug!("on_key_release: cmd = {:?}", cmd);
        self.process_command(cmd)?;
        Ok(())
    }

    fn on_button_press(&mut self, e: ButtonPressEvent) -> Result<()> {
        // Focus the window just clicked.
        if let Some(win) = self.window_mut(e.child) {
            win.focus()?;
            self.focus_changed()?;
        }

        if e.state & u16::from(ModMask::M1) > 0 {
            // button + Alt

            self.ctx
                .conn
                .allow_events(Allow::SYNC_POINTER, x11rb::CURRENT_TIME)?;

            let owner = unwrap_or_return!(self.container_of_mut(e.child));
            if owner.background().contains(e.child) {
                return Ok(());
            }

            let win = unwrap_or_return!(owner.window_mut(e.child));
            let wid = win.frame();
            let geo = self.ctx.conn.get_geometry(wid)?.reply()?;

            let screen = unwrap_or_return!(self.container_of_mut(wid));
            let mon_info = unwrap_or_return!(screen.monitor().map(|mon| &mon.info));
            let mon_x = mon_info.x;
            let mon_y = mon_info.y;

            let rel_x = geo.x - mon_x;
            let rel_y = geo.y - mon_y;

            let win = screen.window_mut(wid).unwrap();
            win.float(Rectangle {
                x: rel_x,
                y: rel_y,
                width: geo.width,
                height: geo.height,
            })?;

            self.drag = Some(MouseDrag {
                wid,
                state: e.state,
                start_x: e.root_x,
                start_y: e.root_y,
                window_x: geo.x,
                window_y: geo.y,
                window_w: geo.width,
                window_h: geo.height,
            });

            self.refresh_layout()?;
            Ok(())
        } else {
            self.ctx
                .conn
                .allow_events(Allow::REPLAY_POINTER, x11rb::CURRENT_TIME)?;
            Ok(())
        }
    }

    fn on_motion_notify(&mut self, e: MotionNotifyEvent) -> Result<()> {
        let left_mask: u16 = ButtonMask::M1.into();
        let right_mask: u16 = ButtonMask::M3.into();

        if e.state & u16::from(ModMask::M1) == 0 || e.state & (left_mask | right_mask) == 0 {
            return Ok(());
        }

        let drag = unwrap_or_return!(self.drag.clone());
        let dx = e.root_x - drag.start_x;
        let dy = e.root_y - drag.start_y;

        let win = unwrap_or_return!(self.window_mut(drag.wid));
        if e.state & left_mask > 0 {
            // Left button
            let aux = ConfigureWindowAux::new()
                .x((drag.window_x + dx) as i32)
                .y((drag.window_y + dy) as i32);
            win.configure(&aux)?;
        } else if e.state & right_mask > 0 {
            // Right button
            let w = drag.window_w as i32 + dx as i32;
            let h = drag.window_h as i32 + dy as i32;
            let w = std::cmp::max(w, 1);
            let h = std::cmp::max(h, 1);

            let aux = ConfigureWindowAux::new().width(w as u32).height(h as u32);
            win.configure(&aux)?;
        }
        Ok(())
    }

    fn on_button_release(&mut self, _: ButtonReleaseEvent) -> Result<()> {
        let drag = unwrap_or_return!(self.drag.take());
        let wid = drag.wid;

        let geo = self.ctx.conn.get_geometry(wid)?.reply()?;

        let screen = unwrap_or_return!(self.container_of_mut(wid));
        let mon = unwrap_or_return!(screen.monitor());
        let mon_info = mon.info.clone();

        let win = screen.window_mut(wid).unwrap();
        win.set_float_geometry(Rectangle {
            x: geo.x - mon_info.x,
            y: geo.y - mon_info.y,
            width: geo.width,
            height: geo.height,
        });

        // TODO: move the ownership of the window to appropriate screen

        self.refresh_layout()?;

        Ok(())
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<()> {
        if req.parent == self.ctx.root {
            let wid = req.window;
            if self.window_mut(wid).is_some() {
                return Ok(());
            }

            let attr = self.ctx.conn.get_window_attributes(wid)?.reply()?;
            if attr.class == WindowClass::INPUT_ONLY {
                return Ok(());
            }

            let screen_id = self.focused_screen_mut()?.id;

            let border_width = self.ctx.config.border.width;
            let mut win = Window::new(self.ctx.clone(), wid, WindowState::Created, border_width)?;
            win.map()?;

            self.screens[screen_id].add_window(win)?;
        } else {
            let win = unwrap_or_return!(self.window_mut(req.parent));
            win.on_map_request(req)?;
        }
        Ok(())
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<()> {
        if notif.override_redirect {
            return Ok(());
        }

        let win = unwrap_or_return!(self.window_mut(notif.event));
        win.on_map_notify(notif)?;
        self.focus_changed()?;
        Ok(())
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<()> {
        let win = unwrap_or_return!(self.window_mut(notif.event));
        win.on_unmap_notify(notif)?;
        self.focus_changed()?;
        Ok(())
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<()> {
        let screen = unwrap_or_return!(self.container_of_mut(notif.window));
        let _ = screen.forget_window(notif.window)?;
        self.focus_changed()?;
        Ok(())
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<()> {
        if req.parent == self.ctx.root {
            let aux = ConfigureWindowAux::from_configure_request(&req);
            self.ctx.conn.configure_window(req.window, &aux)?;
        } else {
            let win = unwrap_or_return!(self.window_mut(req.parent));
            win.on_configure_request(req)?;
        }
        Ok(())
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<()> {
        if notif.override_redirect {
            return Ok(());
        }

        let win = unwrap_or_return!(self.window_mut(notif.event));
        win.on_configure_notify(notif)?;
        Ok(())
    }

    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        let screen = unwrap_or_return!(self.container_of_mut(ev.window));
        screen.on_expose(ev)?;
        Ok(())
    }

    fn on_focus_in(&mut self, focus_in: FocusInEvent) -> Result<()> {
        if focus_in.event == self.ctx.root
            && (focus_in.detail == NotifyDetail::POINTER_ROOT
                || focus_in.detail == NotifyDetail::NONE)
        {
            // Focus the first monitor
            let screen = self.screen_mut_by_mon(0);
            screen.focus_any()?;
        }
        Ok(())
    }

    fn on_client_message(&mut self, ev: ClientMessageEvent) -> Result<()> {
        let name = self.ctx.conn.get_atom_name(ev.type_)?.reply()?.name;
        debug!("ClientMessageEvent.type_: {:?}", String::from_utf8(name));

        if ev.window == self.ctx.root {
            return Ok(());
        }

        let win = unwrap_or_return!(self.window_mut(ev.window));
        win.on_client_message(ev)?;
        Ok(())
    }

    fn on_randr_notify(&mut self, notif: randr::NotifyEvent) -> Result<()> {
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
        Ok(())
    }
}
