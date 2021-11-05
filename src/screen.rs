use log::debug;
use std::collections::BTreeMap;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window as Wid, *};

use crate::context::Context;
use crate::error::Result;
use crate::event::{EventHandlerMethods, HandleResult};
use crate::layout::{self, Layout};
use crate::window::{Window, WindowState};
use crate::winman::Monitor;

#[derive()]
pub struct Screen {
    ctx: Context,
    pub id: usize,
    monitor: Option<Monitor>,
    wins: BTreeMap<Wid, Window>,
    background: Window, // background window
    bar: Window,        // the status bar window
    bar_gc: Gcontext,   // used for drawings on the bar
    layouts: Vec<Box<dyn Layout>>,
    current_layout: usize,
    border_visible: bool,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "Screen {{ id: {}, monitor: {:#?}, wins: {:#?}, background: {}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background.id(), self.layouts[self.current_layout].name(), self.border_visible)
        } else {
            write!(f, "Screen {{ id: {}, monitor: {:?}, wins: {:?}, background: {}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background.id(), self.layouts[self.current_layout].name(), self.border_visible)
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
            Window::new(ctx.clone(), wid, WindowState::Unmapped)?
        };

        let bar = {
            let wid = ctx.conn.generate_id()?;
            let aux = CreateWindowAux::new()
                .background_pixel(0x242424)
                .event_mask(EventMask::EXPOSURE)
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
            Window::new(ctx.clone(), wid, WindowState::Unmapped)?
        };

        let bar_gc = {
            let font = ctx.conn.generate_id()?;
            ctx.conn.open_font(font, b"*")?.check()?;

            let gc = ctx.conn.generate_id()?;
            let aux = CreateGCAux::new()
                .font(font)
                .background(0x242424)
                .foreground(0xFFFFFF);
            ctx.conn.create_gc(gc, bar.id(), &aux)?;
            ctx.conn.close_font(font)?;
            gc
        };

        let mut layouts: Vec<Box<dyn Layout>> = Vec::new();

        // let horizontal = layout::Horizontal::new(ctx.clone());
        // layouts.push(Box::new(horizontal));

        let horizontal = layout::HorizontalWithBorder::new(ctx.clone());
        layouts.push(Box::new(horizontal));

        // let vertical = layout::Vertical::new(ctx.clone());
        // layouts.push(Box::new(vertical));

        let vertical = layout::VerticalWithBorder::new(ctx.clone());
        layouts.push(Box::new(vertical));

        let full = layout::FullScreen::new(ctx.clone());
        layouts.push(Box::new(full));

        assert!(!layouts.is_empty());
        let current_layout = 0;

        Ok(Self {
            ctx,
            id,
            monitor: None,
            wins: Default::default(),
            background,
            bar,
            bar_gc,
            layouts,
            current_layout,
            border_visible: false,
        })
    }

    pub fn attach(&mut self, monitor: Monitor) -> Result<()> {
        debug!(
            "screen.attach: id={}, background={}, monitor={:?}, wins={:?}",
            self.id,
            self.background.id(),
            monitor,
            self.wins
        );

        self.monitor = Some(monitor);
        self.update_background()?;

        self.background.map()?;
        self.bar.map()?;

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
            self.id,
            self.background.id(),
            self.monitor,
            self.wins
        );

        self.background.unmap()?;
        self.bar.unmap()?;

        for w in self.wins.values_mut() {
            if w.is_mapped() {
                w.hide()?;
            }
        }

        Ok(self.monitor.take())
    }

    pub fn update_background(&mut self) -> Result<()> {
        let mon = self.monitor.as_ref().unwrap();

        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(mon.info.height as u32)
            .stack_mode(StackMode::BELOW);
        let id = self.background.id();
        self.ctx.conn.configure_window(id, &aux)?;

        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(16)
            .stack_mode(StackMode::ABOVE);
        let id = self.bar.id();
        self.ctx.conn.configure_window(id, &aux)?;
        self.ctx.conn.flush()?;

        let mut status = String::new();
        for i in 0..5 {
            if i == self.id {
                status += &format!("[{}]", i);
            } else {
                status += &format!(" {} ", i);
            }
        }
        self.ctx
            .conn
            .image_text8(self.bar.id(), self.bar_gc, 0, 13, status.as_bytes())?;
        self.ctx.conn.flush()?;
        Ok(())
    }

    pub fn monitor(&self) -> Option<&Monitor> {
        self.monitor.as_ref()
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
        debug!("screen.forget_window: id={}, wid={}", self.id, wid);

        let mut need_focus_change = false;
        if let Some(focused) = self.ctx.get_focused_window()? {
            if self.window_mut(focused).is_some() {
                need_focus_change = true
            }
        }

        let wid = self.window_mut(wid).expect("unknown window").id();
        let win = self.wins.remove(&wid).expect("unknown window");

        if need_focus_change {
            self.focus_next()?;
        }

        self.refresh_layout()?;
        Ok(win)
    }

    pub fn next_layout(&mut self) -> Result<()> {
        self.current_layout = (self.current_layout + 1) % self.layouts.len();
        self.refresh_layout()
    }

    pub fn refresh_layout(&mut self) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        debug!("screen.refresh_layout: id={}", self.id);

        let mut wins: Vec<&Window> = self.wins.values().filter(|win| win.is_mapped()).collect();
        wins.sort_unstable_by_key(|w| w.id());

        let mon = self.monitor.as_ref().unwrap();
        let mut mon_info = mon.info.clone();

        // make a space for the bar
        mon_info.y += 16;
        mon_info.height -= 16;

        let layout = &mut self.layouts[self.current_layout];
        layout.layout(&mon_info, &wins, self.border_visible)?;

        // update highlight
        {
            let focused = self
                .ctx
                .get_focused_window()?
                .unwrap_or_else(|| InputFocus::NONE.into());

            for win in self.wins.values_mut() {
                if !win.is_mapped() {
                    continue;
                }

                if win.contains(focused) {
                    win.highlight()?;
                } else {
                    win.clear_highlight()?;
                }
            }
        }

        Ok(())
    }

    pub fn background(&self) -> &Window {
        &self.background
    }

    pub fn contains(&self, wid: Wid) -> bool {
        self.background.contains(wid)
            || self.bar.contains(wid)
            || self.wins.contains_key(&wid)
            || self.wins.values().any(|win| win.contains(wid))
    }

    pub fn window_mut(&mut self, wid: Wid) -> Option<&mut Window> {
        if self.background.contains(wid) {
            Some(&mut self.background)
        } else if self.bar.contains(wid) {
            Some(&mut self.bar)
        } else {
            self.wins.values_mut().find(|win| win.contains(wid))
        }
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
                self.background.focus()?;
                Ok(())
            }
        }
    }

    pub fn focus_next(&mut self) -> Result<()> {
        let old = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());

        if !self.contains(old) || self.background.contains(old) {
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
    fn on_expose(&mut self, _ev: ExposeEvent) -> Result<HandleResult> {
        self.update_background()?;
        Ok(HandleResult::Consumed)
    }
}
