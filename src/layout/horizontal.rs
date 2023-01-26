#![allow(dead_code)]

use x11rb::protocol::{randr::MonitorInfo, xproto::*};

use super::Layout;
use crate::context::Context;
use crate::error::Result;
use crate::window::Window;

#[derive(Debug)]
pub struct Horizontal {
    ctx: Context,
    ratio: u16,
}

impl Horizontal {
    pub fn new(ctx: Context) -> Self {
        Self { ctx, ratio: 50 }
    }
}

impl Layout for Horizontal {
    fn name(&self) -> &'static str {
        "horizontal"
    }

    fn layout(
        &mut self,
        mon: &MonitorInfo,
        windows: &mut [&mut Window],
        border_visible: bool,
    ) -> Result<()> {
        if windows.is_empty() {
            return Ok(());
        }

        let offset_x = mon.x as i32;
        let offset_y = mon.y as i32;
        let h = mon.height as u32;

        let border_conf = self.ctx.config.border;
        let border_width = if border_visible { border_conf.width } else { 0 };

        let main_w;
        let w;
        if windows.len() > 1 {
            main_w = mon.width as u32 * self.ratio as u32 / 100;
            w = (mon.width as u32 - main_w) / (windows.len() as u32 - 1);
        } else {
            main_w = mon.width as u32;
            w = 0;
        }
        let mut x = 0;

        // main area
        {
            let conf = ConfigureWindowAux::new()
                .x(offset_x + x)
                .y(offset_y)
                .border_width(border_width)
                .width(main_w - border_width * 2)
                .height(h - border_width * 2);
            windows[0].configure(&conf)?;
            x += main_w as i32;
        }

        for win in windows[1..].iter_mut() {
            let conf = ConfigureWindowAux::new()
                .x(offset_x + x)
                .y(offset_y)
                .border_width(border_width)
                .width(w - border_width * 2)
                .height(h - border_width * 2);
            win.configure(&conf)?;
            x += w as i32;
        }

        Ok(())
    }

    fn process_command(&mut self, cmd: String) -> Result<()> {
        match cmd.as_str() {
            "+" => {
                if self.ratio < 95 {
                    self.ratio += 5;
                }
            }

            "-" => {
                if self.ratio > 5 {
                    self.ratio -= 5;
                }
            }

            _ => {}
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

    fn layout(&mut self, mon: &MonitorInfo, windows: &mut [&mut Window], _: bool) -> Result<()> {
        self.base.layout(mon, windows, true)
    }

    fn process_command(&mut self, cmd: String) -> Result<()> {
        self.base.process_command(cmd)
    }
}
