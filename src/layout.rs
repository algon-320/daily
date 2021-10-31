#![allow(dead_code)]

use crate::context::Context;
use crate::error::Result;

use x11rb::protocol::{randr::MonitorInfo, xproto::*};
use Window as Wid;

pub trait Layout {
    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid], border_visible: bool) -> Result<()>;

    fn name(&self) -> &'static str;
}

#[derive(Debug)]
pub struct HorizontalLayout {
    ctx: Context,
}

impl HorizontalLayout {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }
}

impl Layout for HorizontalLayout {
    fn name(&self) -> &'static str {
        "horizontal"
    }

    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid], border_visible: bool) -> Result<()> {
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
            let border_conf = self.ctx.config.border;
            let border_width = if border_visible { border_conf.width } else { 0 };

            let conf = ConfigureWindowAux::new()
                .x(offset_x + x)
                .y(offset_y)
                .border_width(border_width)
                .width(w - border_width * 2)
                .height(h - border_width * 2);
            self.ctx.conn.configure_window(wid, &conf)?;
            x += w as i32;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct HorizontalLayoutWithBorder {
    ctx: Context,
    base: HorizontalLayout,
}

impl HorizontalLayoutWithBorder {
    pub fn new(ctx: Context) -> Self {
        let base = HorizontalLayout::new(ctx.clone());
        Self { ctx, base }
    }
}

impl Layout for HorizontalLayoutWithBorder {
    fn name(&self) -> &'static str {
        "horizontal-with-border"
    }

    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid], _: bool) -> Result<()> {
        self.base.layout(mon, windows, true)
    }
}
