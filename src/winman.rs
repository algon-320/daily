use log::{debug, info, warn};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandlerMethods, HandleResult};
use crate::screen::{Screen, WindowState};
use crate::{Command, KeybindAction};

use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
    xtest::ConnectionExt as _,
};
use Window as Wid;

const MAX_SCREENS: usize = 10;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub id: usize,
    pub info: randr::MonitorInfo,
}

#[derive()]
pub struct WinMan {
    ctx: Context,
    screens: Vec<Screen>,
    monitor_num: usize,
    focused_screen: usize,
}

impl WinMan {
    pub fn new(ctx: Context) -> Result<Self> {
        let mut wm = Self {
            ctx,
            screens: Vec::new(),
            monitor_num: 0,
            focused_screen: 0,
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

            let state = if attr.map_state == MapState::VIEWABLE {
                WindowState::Mapped
            } else {
                WindowState::Unmapped
            };

            first.add_window(wid, state)?;
        }

        // Focus the first monitor
        self.focused_screen = 0;
        self.focused_screen_mut().focus_any()?;

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

    fn focused_screen_mut(&mut self) -> &mut Screen {
        self.screens.get_mut(self.focused_screen).unwrap()
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
        self.find_screen_mut(|screen| screen.contains(wid).is_some())
    }

    fn change_focus(&mut self, wid: Wid) -> Result<()> {
        debug!("change_focus: wid={}", wid);
        self.ctx.focus_window(wid)?;
        self.focus_changed()?;
        Ok(())
    }

    fn focus_changed(&mut self) -> Result<()> {
        let focus = self.ctx.get_focused_window()?;
        debug!("focused_win = {:?}", focus);
        if let Some(focused_win) = focus {
            if let Some(screen_id) = self
                .screens
                .iter()
                .position(|screen| screen.contains(focused_win).is_some())
            {
                self.focused_screen = screen_id;
            }
        }
        self.refresh_layout()?;
        Ok(())
    }

    fn switch_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        debug!("switch to screen: {}", id);
        if self.screens[id].monitor().is_none() {
            let old = self.focused_screen_mut();
            let mon_info = old.detach()?.expect("focus inconsistent");
            let new = self.screens.get_mut(id).unwrap();
            new.attach(mon_info)?;
        } else {
            let a = self.focused_screen;
            let b = self.screens.iter().position(|sc| sc.id == id).unwrap();

            // no swap needed
            if a == b {
                return Ok(());
            }

            // perfom swap
            use std::cmp::{max, min};
            let (a, b) = (min(a, b), max(a, b));

            let (_, scrs) = self.screens.split_at_mut(a);
            let (screen_a, screen_b) = scrs.split_at_mut(b - a);
            let screen_a = &mut screen_a[0];
            let screen_b = &mut screen_b[0];

            let mon_a = screen_a.detach()?.expect("focus inconsistent");
            let mon_b = screen_b.detach()?.expect("focus inconsistent");

            screen_a.attach(mon_b)?;
            screen_b.attach(mon_a)?;
        }

        self.focused_screen = id;
        self.focused_screen_mut().focus_any()?;
        self.focus_changed()?;
        Ok(())
    }

    fn move_window_to_screen(&mut self, id: usize) -> Result<()> {
        debug_assert!(id < MAX_SCREENS);

        if let Some(wid) = self.ctx.get_focused_window()? {
            debug!("move_window_to_screen: wid = {}", wid);
            let screen = self.focused_screen_mut();

            let state = screen.forget_window(wid)?;
            screen.focus_any()?;
            self.focus_changed()?;

            if let Some(screen) = self.screens.get_mut(id) {
                screen.add_window(wid, state)?;
                return Ok(());
            }
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

    fn launch_app(&self, cmd: &str) -> Result<()> {
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
                    self.ctx.conn.destroy_window(focused)?;
                }
            }

            Command::FocusNext => {
                self.focused_screen_mut().focus_next()?;
                self.focus_changed()?;
            }
            Command::FocusPrev => {
                warn!("Command::FocusPrev: not yet implemented");
            }

            Command::FocusNextMonitor => {
                let focused_monitor = self
                    .focused_screen_mut()
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
                    .focused_screen_mut()
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

            Command::OpenLauncher => self.launch_app(&self.ctx.config.launcher)?,
            Command::OpenTerminal => self.launch_app(&self.ctx.config.terminal)?,

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

            Command::MovePointerUp => self.move_pointer(0, -32)?,
            Command::MovePointerDown => self.move_pointer(0, 32)?,
            Command::MovePointerLeft => self.move_pointer(-32, 0)?,
            Command::MovePointerRight => self.move_pointer(32, 0)?,

            Command::MovePointerUpLittle => self.move_pointer(0, -1)?,
            Command::MovePointerDownLittle => self.move_pointer(0, 1)?,
            Command::MovePointerLeftLittle => self.move_pointer(-1, 0)?,
            Command::MovePointerRightLittle => self.move_pointer(1, 0)?,

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
        if e.child != x11rb::NONE && self.screens.iter().any(|s| s.contains(e.child).is_some()) {
            self.change_focus(e.child)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let screen = self.focused_screen_mut();
            screen.add_window(notif.window, WindowState::Created)?;

            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        self.ctx.conn.map_window(req.window)?;
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
            let res = screen.on_destroy_notify(notif);
            self.focus_changed()?;
            return res;
        }
        Ok(HandleResult::Ignored)
    }

    fn on_configure_notify(&mut self, _notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        Ok(HandleResult::Ignored)
    }

    fn on_focus_in(&mut self, focus_in: FocusInEvent) -> Result<HandleResult> {
        if focus_in.event == self.ctx.root {
            if focus_in.detail == NotifyDetail::POINTER_ROOT {
                self.focused_screen_mut().focus_any()?;
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
