#![allow(dead_code)]

use x11rb::protocol::{randr::MonitorInfo, xproto::*};

use super::Layout;
use crate::context::Context;
use crate::error::Result;
use crate::window::Window;

#[derive(Debug)]
pub struct FullScreen {
    ctx: Context,
}

impl FullScreen {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }
}

impl Layout for FullScreen {
    fn name(&self) -> &'static str {
        "full-screen"
    }

    fn layout(
        &mut self,
        mon: &MonitorInfo,
        windows: &[&Window],
        _border_visible: bool,
    ) -> Result<()> {
        if windows.is_empty() {
            return Ok(());
        }

        let x = mon.x as i32;
        let y = mon.y as i32;
        let w = mon.width as u32;
        let h = mon.height as u32;

        let base_conf = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .border_width(0)
            .width(w)
            .height(h);

        let focus = self
            .ctx
            .get_focused_window()?
            .unwrap_or_else(|| InputFocus::NONE.into());

        for &win in windows.iter() {
            let conf = if win.contains(focus) {
                base_conf.stack_mode(StackMode::ABOVE) // Top-most
            } else {
                base_conf
            };
            self.ctx.conn.configure_window(win.frame(), &conf)?;
        }

        Ok(())
    }
}
