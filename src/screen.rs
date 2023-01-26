use log::debug;
use std::collections::{BTreeMap, VecDeque};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window as Wid, *};

use crate::bar::Content;
use crate::context::Context;
use crate::error::Result;
use crate::event::EventHandlerMethods;
use crate::layout::{self, Layout};
use crate::monitor::Monitor;
use crate::window::{Window, WindowState};

#[derive()]
pub struct Screen {
    ctx: Context,
    pub id: usize,
    monitor: Option<Monitor>,
    wins: BTreeMap<Wid, Window>,
    background: Window,
    layouts: VecDeque<Box<dyn Layout>>,
    border_visible: bool,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "Screen {{ id: {}, monitor: {:#?}, wins: {:#?}, background: {:#?}, layout: {}, border_visible: {} }}",
                self.id, self.monitor, self.wins, self.background, self.layouts.front().unwrap().name(), self.border_visible)
        } else {
            write!(f, "Screen {{ id: {}, monitor: {:?}, wins: {:?}, background: {:?}, layout: {}, border_visible: {} }}",
                self.id, self.monitor, self.wins, self.background, self.layouts.front().unwrap().name(), self.border_visible)
        }
    }
}

impl Screen {
    pub fn new(ctx: Context, id: usize) -> Result<Self> {
        let background = {
            let wid = ctx.conn.generate_id()?;
            let depth = x11rb::COPY_DEPTH_FROM_PARENT;
            let class = WindowClass::INPUT_OUTPUT;
            let visual = x11rb::COPY_FROM_PARENT;
            let aux = CreateWindowAux::new()
                .background_pixel(ctx.config.background_color)
                .event_mask(EventMask::FOCUS_CHANGE);
            ctx.conn
                .create_window(depth, wid, ctx.root, 0, 0, 16, 16, 0, class, visual, &aux)?;
            Window::new(ctx.clone(), wid, WindowState::Unmapped, 0)?
        };

        let mut layouts: VecDeque<Box<dyn Layout>> = VecDeque::new();

        // let horizontal = layout::Horizontal::new(ctx.clone());
        // layouts.push_back(Box::new(horizontal));

        let horizontal = layout::HorizontalWithBorder::new(ctx.clone());
        layouts.push_back(Box::new(horizontal));

        // let vertical = layout::Vertical::new(ctx.clone());
        // layouts.push_back(Box::new(vertical));

        let vertical = layout::VerticalWithBorder::new(ctx.clone());
        layouts.push_back(Box::new(vertical));

        let full = layout::FullScreen::new(ctx.clone());
        layouts.push_back(Box::new(full));

        assert!(!layouts.is_empty());

        Ok(Self {
            ctx,
            id,
            monitor: None,
            wins: Default::default(),
            background,
            layouts,
            border_visible: false,
        })
    }

    pub fn attach(&mut self, monitor: Monitor) -> Result<()> {
        debug!(
            "screen.attach: id={}, background={:?}, monitor={:?}, wins={:?}",
            self.id, self.background, monitor, self.wins
        );

        self.monitor = Some(monitor);
        self.update()?;

        self.background.map()?;
        for win in self.wins.values_mut() {
            win.show()?;
        }

        Ok(())
    }

    pub fn detach(&mut self) -> Result<Option<Monitor>> {
        if self.monitor.is_none() {
            return Ok(None);
        }

        debug!(
            "screen.detach: id={}, background={:?}, monitor={:?}, wins={:?}",
            self.id, self.background, self.monitor, self.wins
        );

        self.background.unmap()?;
        for w in self.wins.values_mut() {
            w.hide()?;
        }

        Ok(self.monitor.take())
    }

    pub fn swap_monitors(a: &mut Self, b: &mut Self) -> Result<()> {
        std::mem::swap(&mut a.monitor, &mut b.monitor);
        a.update()?;
        b.update()?;
        Ok(())
    }

    fn update(&mut self) -> Result<()> {
        let focused_window = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());
        let focused = self.contains(focused_window);

        // update the bar
        let mon = self.monitor.as_mut().expect("monitor is not attached");
        mon.bar
            .configure(mon.info.x, mon.info.y, mon.info.width, 16)
            .expect("TODO: bar.configure");
        mon.bar
            .update_content(Content {
                max_screen: self.ctx.config.screens,
                current_screen: self.id,
                focused,
            })
            .expect("TODO: bar.update_content");

        // update the background
        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(mon.info.height as u32)
            .stack_mode(StackMode::BELOW);
        self.background.configure(&aux)?;

        Ok(())
    }

    pub fn monitor(&self) -> Option<&Monitor> {
        self.monitor.as_ref()
    }

    pub fn add_window(&mut self, mut win: Window) -> Result<()> {
        if self.wins.contains_key(&win.frame()) {
            return Ok(());
        }

        debug!("add_window: win={:?}", win);

        if self.monitor.is_none() && win.is_mapped() {
            win.hide()?;
        }

        // Float the window if it is a dialog
        let type_dialog = self.ctx.atom._NET_WM_WINDOW_TYPE_DIALOG;
        if win.net_wm_type()? == Some(type_dialog) {
            let geo = self.ctx.conn.get_geometry(win.frame())?.reply()?;

            let x;
            let y;
            if let Some(mon) = self.monitor.as_ref() {
                x = (mon.info.width / 2) as i16 - (geo.width / 2) as i16;
                y = (mon.info.height / 2) as i16 - (geo.height / 2) as i16;
            } else {
                x = 0;
                y = 0;
            }

            win.float(Rectangle {
                x,
                y,
                width: geo.width,
                height: geo.height,
            })?;
        }

        self.wins.insert(win.frame(), win);
        self.refresh_layout()?;
        Ok(())
    }

    pub fn forget_window(&mut self, wid: Wid) -> Result<Window> {
        debug!("screen.forget_window: id={}, wid={:08X}", self.id, wid);

        let mut need_focus_change = false;
        if let Some(focused) = self.ctx.get_focused_window()? {
            if self.window_mut(focused).is_some() {
                need_focus_change = true
            }
        }

        let wid = self.window_mut(wid).expect("unknown window").frame();
        let win = self.wins.remove(&wid).expect("unknown window");

        if need_focus_change {
            self.focus_next()?;
        }

        self.refresh_layout()?;
        Ok(win)
    }

    pub fn next_layout(&mut self) -> Result<()> {
        self.layouts.rotate_left(1);
        self.refresh_layout()
    }

    pub fn refresh_layout(&mut self) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        debug!("screen.refresh_layout: id={}", self.id);

        let mon = self.monitor.as_ref().unwrap();

        // for normal mapped windows
        {
            let mut wins: Vec<&mut Window> = self
                .wins
                .values_mut()
                .filter(|win| win.is_mapped() && !win.is_floating())
                .collect();
            wins.sort_unstable_by_key(|w| w.frame());

            let mut mon_info = mon.info.clone();

            let layout = self.layouts.front_mut().expect("no layout");

            // make a space for the bar
            if layout.name() != "full-screen" {
                mon_info.y += 16;
                mon_info.height -= 16;
            }

            layout.layout(&mon_info, &mut wins, self.border_visible)?;
        }

        // for floating windows
        {
            for win in self
                .wins
                .values_mut()
                .filter(|win| win.is_mapped() && win.is_floating())
            {
                let geo = win.get_float_geometry().unwrap();
                let aux = ConfigureWindowAux::new()
                    .x((mon.info.x + geo.x) as i32)
                    .y((mon.info.y + geo.y) as i32)
                    .width(geo.width as u32)
                    .height(geo.height as u32);
                win.configure(&aux)?;
            }
        }

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

                let highlight = win.contains(focused);
                win.set_highlight(highlight)?;
            }
        }

        self.update()?;

        Ok(())
    }

    pub fn background(&self) -> &Window {
        &self.background
    }

    pub fn contains(&self, wid: Wid) -> bool {
        self.background.contains(wid)
            || self.wins.contains_key(&wid)
            || self.wins.values().any(|win| win.contains(wid))
    }

    pub fn window_mut(&mut self, wid: Wid) -> Option<&mut Window> {
        if self.background.contains(wid) {
            Some(&mut self.background)
        } else {
            self.wins.values_mut().find(|win| win.contains(wid))
        }
    }

    pub fn focus_any(&mut self) -> Result<()> {
        debug!("screen {}: focus_any", self.id);
        match self.wins.values_mut().find(|win| win.is_mapped()) {
            Some(first) => {
                first.focus()?;
            }
            None => {
                debug!("screen {}: focus background", self.id);
                self.background.focus()?;
            }
        }
        Ok(())
    }

    pub fn focus_next(&mut self) -> Result<()> {
        let old = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());

        if !self.contains(old) || self.background.contains(old) {
            return self.focus_any();
        }

        let old = self.window_mut(old).unwrap().frame();

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

    pub fn alarm(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn layout_command(&mut self, cmd: String) -> Result<()> {
        let layout = self.layouts.front_mut().expect("no layout");
        layout.process_command(cmd)?;
        self.refresh_layout()?;
        Ok(())
    }
}

impl EventHandlerMethods for Screen {
    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        let wid = ev.window;
        assert!(self.contains(wid));
        if let Some(win) = self.window_mut(wid) {
            win.on_expose(ev)?;
        }
        Ok(())
    }
}
