use crate::error::Result;
use crate::Context;

use x11rb::connection::Connection;
use x11rb::protocol::{randr::MonitorInfo, xproto::*};
use Window as Wid;

pub trait Layout {
    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid]) -> Result<()>;
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
    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid]) -> Result<()> {
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
            let border = self.ctx.config.border.clone();
            let conf = ConfigureWindowAux::new()
                .x(offset_x + x)
                .y(offset_y)
                .border_width(border.width)
                .width(w - border.width * 2)
                .height(h - border.width * 2);
            self.ctx.conn.configure_window(wid, &conf)?;
            x += w as i32;
        }
        self.ctx.conn.flush()?;

        Ok(())
    }
}
