use log::debug;
use std::collections::BTreeMap;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window as Wid, *};

use crate::context::Context;
use crate::error::Result;
use crate::event::EventHandlerMethods;
use crate::layout::{self, Layout};
use crate::window::{Window, WindowState};
use crate::winman::Monitor;

fn draw_digit<C: Connection>(
    conn: &C,
    wid: Drawable,
    gc: Gcontext,
    x: i16,
    y: i16,
    ascii_digit: u8,
    color1: u32,
    color2: u32,
) -> Result<()> {
    const DIGITS: [[u32; 6 * 6]; 10 + 3] = include!("digits.txt");

    let digit = if (b'0'..=b'9').contains(&ascii_digit) {
        ascii_digit - b'0'
    } else if ascii_digit == b':' {
        10
    } else if ascii_digit == b'/' {
        11
    } else if ascii_digit == b' ' {
        12
    } else {
        panic!(
            "unsupported char: {}",
            char::from_u32(ascii_digit as u32).unwrap()
        );
    };

    let mut ps1 = Vec::new();
    let mut ps2 = Vec::new();
    for (p, &e) in DIGITS[digit as usize].iter().enumerate() {
        let (yi, xi) = (p / 6, p % 6);
        let point = Point {
            x: x + xi as i16,
            y: y + yi as i16,
        };
        if e == 1 {
            ps1.push(point);
        } else if e == 2 {
            ps2.push(point);
        }
    }

    if !ps1.is_empty() {
        let aux = ChangeGCAux::new().foreground(color1);
        conn.change_gc(gc, &aux)?;
        conn.poly_point(CoordMode::ORIGIN, wid, gc, &ps1)?;
    }

    if !ps2.is_empty() {
        let aux = ChangeGCAux::new().foreground(color2);
        conn.change_gc(gc, &aux)?;
        conn.poly_point(CoordMode::ORIGIN, wid, gc, &ps2)?;
    }

    Ok(())
}

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
            write!(f, "Screen {{ id: {}, monitor: {:#?}, wins: {:#?}, background: {:#?}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background, self.layouts[self.current_layout].name(), self.border_visible)
        } else {
            write!(f, "Screen {{ id: {}, monitor: {:?}, wins: {:?}, background: {:?}, layout: {}, border_visible: {} }}", self.id, self.monitor, self.wins, self.background, self.layouts[self.current_layout].name(), self.border_visible)
        }
    }
}

impl Screen {
    pub fn new(ctx: Context, id: usize) -> Result<Self> {
        let background = {
            let wid = ctx.conn.generate_id()?;
            let aux = CreateWindowAux::new()
                .background_pixel(ctx.config.background_color)
                .event_mask(EventMask::FOCUS_CHANGE);
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
                .background_pixel(0x4e4b61)
                .event_mask(EventMask::EXPOSURE);
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
            // let font = ctx.conn.generate_id()?;
            // ctx.conn.open_font(font, b"fixed")?.check()?;

            let gc = ctx.conn.generate_id()?;
            let aux = CreateGCAux::new()
                // .font(font)
                .background(0x4e4b61)
                .foreground(0xd2ca9c);
            ctx.conn.create_gc(gc, bar.inner(), &aux)?;
            // ctx.conn.close_font(font)?;
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
            "screen.attach: id={}, background={:?}, monitor={:?}, wins={:?}",
            self.id, self.background, monitor, self.wins
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
            "screen.detach: id={}, background={:?}, monitor={:?}, wins={:?}",
            self.id, self.background, self.monitor, self.wins
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

    pub fn swap_monitors(a: &mut Self, b: &mut Self) -> Result<()> {
        std::mem::swap(&mut a.monitor, &mut b.monitor);
        a.update_background()?;
        b.update_background()?;
        Ok(())
    }

    pub fn update_background(&mut self) -> Result<()> {
        let mon = self.monitor.as_ref().expect("monitor is not attached");

        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(mon.info.height as u32)
            .stack_mode(StackMode::BELOW);
        let id = self.background.frame();
        self.ctx.conn.configure_window(id, &aux)?;

        let aux = ConfigureWindowAux::new()
            .x(mon.info.x as i32)
            .y(mon.info.y as i32)
            .width(mon.info.width as u32)
            .height(16)
            .sibling(self.background.frame())
            .stack_mode(StackMode::ABOVE);
        let id = self.bar.frame();
        self.ctx.conn.configure_window(id, &aux)?;

        self.draw_bar()?;

        Ok(())
    }

    fn draw_bar(&mut self) -> Result<()> {
        debug!("screen.draw_bar: id={}", self.id);

        let mon = self.monitor.as_ref().expect("monitor is not attached");
        let w = mon.info.width as i16;

        let bar = self.bar.inner();
        let gc = self.bar_gc;

        let color_bg = 0x4e4b61;

        // Lines
        let aux = ChangeGCAux::new().foreground(0x69656d);
        self.ctx.conn.change_gc(gc, &aux)?;

        let p1 = Point { x: 0, y: 14 };
        let p2 = Point { x: 0, y: 0 };
        let p3 = Point { x: w - 2, y: 0 };
        self.ctx
            .conn
            .poly_line(CoordMode::ORIGIN, bar, gc, &[p1, p2, p3])?;

        let aux = ChangeGCAux::new().foreground(0x1a1949);
        self.ctx.conn.change_gc(gc, &aux)?;

        let p1 = Point { x: 1, y: 15 };
        let p2 = Point { x: w - 1, y: 15 };
        let p3 = Point { x: w - 1, y: 1 };
        self.ctx
            .conn
            .poly_line(CoordMode::ORIGIN, bar, gc, &[p1, p2, p3])?;

        // Digits
        let offset_x = 2;
        let offset_y = 5;
        for i in 0..5 {
            let color1: u32 = if i == self.id { 0x00f080 } else { 0xd2ca9c };
            let color2: u32 = if i == self.id { 0x007840 } else { 0x9d9784 };

            let x = offset_x + (i * 12) as i16;
            let y = offset_y;
            let digit = b'1' + i as u8;
            draw_digit(&self.ctx.conn, bar, gc, x, y, digit, color1, color2)?;
        }

        // clock
        use chrono::prelude::*;
        let mut x = w - 136;
        let y = 5;

        let aux = ChangeGCAux::new().foreground(color_bg).background(color_bg);
        self.ctx.conn.change_gc(gc, &aux)?;

        let rect = Rectangle {
            x,
            y,
            width: (6 + 2) * 16,
            height: 6,
        };
        self.ctx.conn.poly_fill_rectangle(bar, gc, &[rect])?;

        let (color1, color2) = (0xd2ca9c, 0x9d9784);
        let now = chrono::Local::now();
        let date = now.date();
        let time = now.time();

        let date_time = format!(
            "{:04}/{:02}/{:02} {:02}:{:02}",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute()
        );
        for &b in date_time.as_bytes() {
            draw_digit(&self.ctx.conn, bar, gc, x, y, b, color1, color2)?;
            x += 8;
        }

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
        self.current_layout = (self.current_layout + 1) % self.layouts.len();
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
            let mut wins: Vec<&Window> = self
                .wins
                .values()
                .filter(|win| win.is_mapped() && !win.is_floating())
                .collect();
            wins.sort_unstable_by_key(|w| w.frame());

            let mut mon_info = mon.info.clone();

            let layout = &mut self.layouts[self.current_layout];

            // make a space for the bar
            if layout.name() != "full-screen" {
                mon_info.y += 16;
                mon_info.height -= 16;
            }

            layout.layout(&mon_info, &wins, self.border_visible)?;
        }

        // for floating windows
        {
            for win in self
                .wins
                .values()
                .filter(|win| win.is_mapped() && win.is_floating())
            {
                let geo = win.get_float_geometry().unwrap();
                let aux = ConfigureWindowAux::new()
                    .x((mon.info.x + geo.x) as i32)
                    .y((mon.info.y + geo.y) as i32)
                    .width(geo.width as u32)
                    .height(geo.height as u32);
                self.ctx.conn.configure_window(win.frame(), &aux)?;
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

        self.draw_bar()?;

        Ok(())
    }

    pub fn background(&self) -> &Window {
        &self.background
    }

    pub fn bar(&self) -> &Window {
        &self.bar
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

        if !self.contains(old) || self.background.contains(old) || self.bar.contains(old) {
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
}

impl EventHandlerMethods for Screen {
    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        let wid = ev.window;
        assert!(self.contains(wid));
        if self.bar.contains(wid) {
            self.draw_bar()?;
        } else if let Some(win) = self.window_mut(wid) {
            win.on_expose(ev)?;
        }
        Ok(())
    }
}
