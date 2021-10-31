use log::debug;
use std::collections::BTreeMap;

use crate::context::Context;
use crate::error::Result;
use crate::event::{EventHandlerMethods, HandleResult};
use crate::layout::{HorizontalLayout, Layout};
use crate::window::{Window, WindowState};
use crate::winman::Monitor;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window as Wid, *};

#[derive()]
pub struct Screen {
    ctx: Context,
    pub id: usize,
    monitor: Option<Monitor>,
    wins: BTreeMap<Wid, Window>,
    background: Wid, // background window
    layout: Box<dyn Layout>,
    border_visible: bool,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "Screen {{ id: {}, monitor: {:#?}, wins: {:#?}, background: {}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background, self.layout.name(), self.border_visible)
        } else {
            write!(f, "Screen {{ id: {}, monitor: {:?}, wins: {:?}, background: {}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background, self.layout.name(), self.border_visible)
        }
    }
}

impl Screen {
    pub fn new(ctx: Context, id: usize) -> Result<Self> {
        let background = {
            let wid = ctx.conn.generate_id()?;
            let aux = CreateWindowAux::new()
                .background_pixel(ctx.config.background_color)
                .event_mask(EventMask::FOCUS_CHANGE)
                .override_redirect(1); // special window
            ctx.conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                wid,
                ctx.root,
                0,  // x
                0,  // y
                16, // w
                16, // h
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )?;
            wid
        };

        let layout = HorizontalLayout::new(ctx.clone());

        Ok(Self {
            ctx,
            id,
            monitor: None,
            wins: Default::default(),
            background,
            layout: Box::new(layout),
            border_visible: false,
        })
    }

    pub fn attach(&mut self, monitor: Monitor) -> Result<()> {
        debug!(
            "screen.attach: id={}, background={}, monitor={:?}, wins={:?}",
            self.id, self.background, monitor, self.wins
        );

        self.monitor = Some(monitor);
        self.update_background()?;

        self.ctx.conn.map_window(self.background)?;
        for win in self.wins.values_mut() {
            let state = win.state();
            if state == WindowState::Mapped || state == WindowState::Hidden {
                win.map()?;
            }
        }

        Ok(())
    }

    pub fn detach(&mut self) -> Result<Option<Monitor>> {
        if self.monitor.is_none() {
            return Ok(None);
        }

        debug!(
            "screen.detach: id={}, background={}, monitor={:?}, wins={:?}",
            self.id, self.background, self.monitor, self.wins
        );

        self.ctx.conn.unmap_window(self.background)?;
        for w in self.wins.values_mut() {
            if w.is_mapped() {
                w.hide()?;
            }
        }

        Ok(self.monitor.take())
    }

    pub fn swap_monitor(screen1: &mut Self, screen2: &mut Self) -> Result<()> {
        assert!(screen1.monitor.is_some() && screen2.monitor.is_some());

        debug!(
            "screen.swap_monitor: id1={}, id2={}, mon1={:?}, mon2={:?}",
            screen1.id,
            screen2.id,
            screen1.monitor.as_ref().unwrap(),
            screen2.monitor.as_ref().unwrap(),
        );

        std::mem::swap(&mut screen1.monitor, &mut screen2.monitor);

        screen1.update_background()?;
        screen2.update_background()?;

        Ok(())
    }

    pub fn update_background(&mut self) -> Result<()> {
        let mon = self.monitor.as_ref().unwrap();
        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(mon.info.height as u32)
            .stack_mode(StackMode::BELOW);
        self.ctx.conn.configure_window(self.background, &aux)?;
        Ok(())
    }

    pub fn monitor(&self) -> Option<Monitor> {
        self.monitor.clone()
    }

    pub fn add_window(&mut self, mut win: Window) -> Result<()> {
        if self.wins.contains_key(&win.id()) {
            return Ok(());
        }

        debug!("add_window: win={:?}", win);

        if self.monitor.is_none() && win.is_mapped() {
            win.hide()?;
        }

        self.wins.insert(win.id(), win);
        self.refresh_layout()?;
        Ok(())
    }

    pub fn forget_window(&mut self, wid: Wid) -> Result<Window> {
        let wid = self.window_mut(wid).expect("unknown window").id();

        debug!("screen.forget_window: id={}, wid={}", self.id, wid);
        let win = self.wins.remove(&wid).expect("unknown window");

        self.refresh_layout()?;
        Ok(win)
    }

    pub fn refresh_layout(&mut self) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        debug!("screen.refresh_layout: id={}", self.id);

        let focused = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());

        let mut wids: Vec<Wid> = self
            .wins
            .iter()
            .filter_map(|(&wid, win)| if win.is_mapped() { Some(wid) } else { None })
            .collect();
        wids.sort_unstable();

        let mon = self.monitor.as_ref().unwrap();
        self.layout.layout(&mon.info, &wids, self.border_visible)?;

        // update highlight
        for win in self.wins.values_mut() {
            if win.contains(focused) {
                debug!("highlight: win={:?}", win);
                win.highlight()?;
            } else {
                win.clear_highlight()?;
            }
        }

        Ok(())
    }

    pub fn background(&self) -> Wid {
        self.background
    }

    pub fn contains(&self, wid: Wid) -> bool {
        if wid == self.background {
            true
        } else {
            self.wins.contains_key(&wid) || self.wins.values().any(|win| win.contains(wid))
        }
    }

    pub fn window_mut(&mut self, wid: Wid) -> Option<&mut Window> {
        self.wins.values_mut().find(|win| win.contains(wid))
    }

    pub fn focus_any(&mut self) -> Result<()> {
        debug!("screen {}: focus_any", self.id);
        match self
            .wins
            .values_mut()
            .find(|win| win.state() == WindowState::Mapped || win.state() == WindowState::Hidden)
        {
            Some(first) => first.focus(),
            None => {
                debug!("screen {}: focus background", self.id);
                self.ctx.focus_window(self.background)
            }
        }
    }

    pub fn focus_next(&mut self) -> Result<()> {
        let old = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());

        if !self.contains(old) || old == self.background {
            return self.focus_any();
        }

        let old = self.window_mut(old).unwrap().id();

        let next = self
            .wins
            .iter()
            .filter(|(_, win)| win.is_mapped())
            .map(|(wid, _)| wid)
            .copied()
            .cycle()
            .skip_while(|&wid| wid != old)
            .nth(1)
            .unwrap();

        if let Some(win) = self.wins.get_mut(&next) {
            debug!("focus_next: next={:?}", win);
            win.focus()?;
        }
        Ok(())
    }

    pub fn show_border(&mut self) {
        self.border_visible = true;
    }
    pub fn hide_border(&mut self) {
        self.border_visible = false;
    }
}

impl EventHandlerMethods for Screen {
    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        let wid = req.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            unreachable!(); // because of overide_redirect
        }

        let win = self.window_mut(wid).unwrap();
        win.on_map_request(req)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            return Ok(HandleResult::Consumed);
        }

        let win = self.window_mut(wid).unwrap();
        win.on_map_notify(notif)?;

        self.refresh_layout()?;
        Ok(HandleResult::Consumed)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            return Ok(HandleResult::Consumed);
        }

        let win = self.window_mut(wid).unwrap();
        win.on_unmap_notify(notif)?;

        self.refresh_layout()?;
        Ok(HandleResult::Consumed)
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            return Ok(HandleResult::Consumed);
        }

        if Some(wid) == self.ctx.get_focused_window()? {
            self.focus_next()?;
        }

        let wid = self.window_mut(wid).unwrap().id();
        let _ = self.forget_window(wid)?;
        Ok(HandleResult::Consumed)
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<HandleResult> {
        let wid = req.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            let aux = ConfigureWindowAux::from_configure_request(&req);
            self.ctx.conn.configure_window(wid, &aux)?;
            return Ok(HandleResult::Consumed);
        }

        let win = self.window_mut(wid).unwrap();
        let res = win.on_configure_request(req);

        self.refresh_layout()?;

        res
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if !self.contains(wid) {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            unreachable!(); // because of override_redirect
        }

        let win = self.window_mut(wid).unwrap();
        win.on_configure_notify(notif)
    }

    fn on_focus_in(&mut self, _focus_in: FocusInEvent) -> Result<HandleResult> {
        Ok(HandleResult::Consumed)
    }

    fn on_focus_out(&mut self, _focus_out: FocusInEvent) -> Result<HandleResult> {
        Ok(HandleResult::Consumed)
    }
}
