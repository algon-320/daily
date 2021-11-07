#![allow(dead_code)]

use x11rb::protocol::{randr::MonitorInfo, xproto::*};

use super::Layout;
use crate::context::Context;
use crate::error::Result;
use crate::window::Window;

#[derive(Debug)]
pub struct Horizontal {
    ctx: Context,
}

impl Horizontal {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }
}

impl Layout for Horizontal {
    fn name(&self) -> &'static str {
        "horizontal"
    }

    fn layout(
        &mut self,
        mon: &MonitorInfo,
        windows: &[&Window],
        border_visible: bool,
    ) -> Result<()> {
        if windows.is_empty() {
            return Ok(());
        }

        let count = windows.len();
        let w = (mon.width / count as u16) as u32;
        let h = mon.height as u32;
        let offset_x = mon.x as i32;
        let offset_y = mon.y as i32;
        let mut x = 0;

        for win in windows.iter() {
            let wid = win.frame();

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
pub struct HorizontalWithBorder {
    base: Horizontal,
}

impl HorizontalWithBorder {
    pub fn new(ctx: Context) -> Self {
        Self {
            base: Horizontal::new(ctx),
        }
    }
}

impl Layout for HorizontalWithBorder {
    fn name(&self) -> &'static str {
        "horizontal-with-border"
    }

    fn layout(&mut self, mon: &MonitorInfo, windows: &[&Window], _: bool) -> Result<()> {
        self.base.layout(mon, windows, true)
    }
}
