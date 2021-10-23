use crate::context::Context;
use crate::error::Result;

use x11rb::connection::Connection;
use x11rb::protocol::{randr::MonitorInfo, xproto::*};
use Window as Wid;

pub trait Layout {
    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid], border_visible: bool) -> Result<()>;
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
    fn layout(&mut self, mon: &MonitorInfo, windows: &[Wid], border_visible: bool) -> Result<()> {
        if windows.is_empty() {
            return Ok(());
        }

        let focused = self.ctx.get_focused_window()?;

        let count = windows.len();
        let w = (mon.width / count as u16) as u32;
        let h = mon.height as u32;
        let offset_x = mon.x as i32;
        let offset_y = mon.y as i32;
        let mut x = 0;

        for &wid in windows.iter() {
            let border_conf = self.ctx.config.border;

            if border_visible {
                let color = if wid == focused {
                    border_conf.color_focused
                } else {
                    border_conf.color_regular
                };
                let attr = ChangeWindowAttributesAux::new().border_pixel(color);
                self.ctx.conn.change_window_attributes(wid, &attr)?;
            }

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
        self.ctx.conn.flush()?;

        Ok(())
    }
}
