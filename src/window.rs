use log::debug;

use crate::context::Context;
use crate::error::Result;
use crate::event::{EventHandlerMethods, HandleResult};

use x11rb::protocol::xproto::{Window as Wid, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Created,
    Mapped,
    Unmapped,
    Hidden,
}

fn frame_window(ctx: &Context, wid: Wid) -> Result<Wid> {
    use x11rb::connection::Connection as _;

    let geo = ctx.conn.get_geometry(wid)?.reply()?;
    let frame = ctx.conn.generate_id()?;
    let aux = CreateWindowAux::new()
        .event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT);
    ctx.conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        frame,
        ctx.root,
        geo.x,
        geo.y,
        geo.width,
        geo.height,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &aux,
    )?;
    ctx.conn.reparent_window(wid, frame, 0, 16)?;

    Ok(frame)
}

#[derive(Clone)]
pub struct Window {
    ctx: Context,
    frame: Option<Wid>,
    inner: Wid,
    state: WindowState,
}

impl Window {
    pub fn new(ctx: Context, inner: Wid, state: WindowState) -> Result<Self> {
        if state == WindowState::Mapped {
            ctx.conn.map_window(inner)?;
        }

        Ok(Self {
            ctx,
            frame: None,
            inner,
            state,
        })
    }

    pub fn new_framed(ctx: Context, inner: Wid, state: WindowState) -> Result<Self> {
        let frame = frame_window(&ctx, inner)?;

        if state == WindowState::Mapped {
            ctx.conn.map_window(inner)?;
            ctx.conn.map_window(frame)?;
        }

        Ok(Self {
            ctx,
            frame: Some(frame),
            inner,
            state,
        })
    }

    pub fn is_mapped(&self) -> bool {
        self.state == WindowState::Mapped
    }

    pub fn state(&self) -> WindowState {
        self.state
    }

    pub fn id(&self) -> Wid {
        self.frame.unwrap_or(self.inner)
    }

    pub fn contains(&self, wid: Wid) -> bool {
        self.inner == wid || self.frame == Some(wid)
    }

    pub fn map(&mut self) -> Result<()> {
        if self.state != WindowState::Mapped {
            if let Some(frame) = self.frame {
                self.ctx.conn.map_window(frame)?;
            }
            self.ctx.conn.map_window(self.inner)?;

            // Newly mapped window
            if self.state == WindowState::Created {
                debug!("focus newly mapped window: win={:?}", self);
                self.focus()?;
            }

            self.state = WindowState::Mapped;
        }
        Ok(())
    }

    pub fn unmap(&mut self) -> Result<()> {
        if self.state != WindowState::Unmapped {
            if let Some(frame) = self.frame {
                self.ctx.conn.unmap_window(frame)?;
            }
            self.ctx.conn.unmap_window(self.inner)?;

            self.state = WindowState::Unmapped;
        }
        Ok(())
    }

    pub fn hide(&mut self) -> Result<()> {
        if self.state != WindowState::Hidden {
            let wid = self.frame.unwrap_or(self.inner);
            self.ctx.conn.unmap_window(wid)?;

            self.state = WindowState::Hidden;
        }
        Ok(())
    }

    pub fn focus(&mut self) -> Result<()> {
        self.ctx.focus_window(self.inner)
    }

    fn paint_background(&mut self, gc: Gcontext) -> Result<()> {
        if let Some(frame) = self.frame {
            let rect = Rectangle {
                x: 0,
                y: 0,
                width: 10000,
                height: 16,
            };
            self.ctx.conn.poly_fill_rectangle(frame, gc, &[rect])?;
        }
        Ok(())
    }

    pub fn highlight(&mut self) -> Result<()> {
        if self.frame.is_some() {
            self.paint_background(self.ctx.color_focused)?;
        } else {
            let color = self.ctx.config.border.color_focused;
            let aux = ChangeWindowAttributesAux::new().border_pixel(color);
            self.ctx.conn.change_window_attributes(self.inner, &aux)?;
        }
        Ok(())
    }

    pub fn clear_highlight(&mut self) -> Result<()> {
        if self.frame.is_some() {
            self.paint_background(self.ctx.color_regular)?;
        } else {
            let color = self.ctx.config.border.color_regular;
            let aux = ChangeWindowAttributesAux::new().border_pixel(color);
            self.ctx.conn.change_window_attributes(self.inner, &aux)?;
        }
        Ok(())
    }
}

impl EventHandlerMethods for Window {
    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        if !self.contains(req.window) {
            return Ok(HandleResult::Ignored);
        }

        self.map()?;
        Ok(HandleResult::Consumed)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !self.contains(notif.window) {
            return Ok(HandleResult::Ignored);
        }

        self.map()?;
        Ok(HandleResult::Consumed)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if !self.contains(notif.window) {
            return Ok(HandleResult::Ignored);
        }

        if self.state == WindowState::Mapped {
            self.unmap()?;
        }

        Ok(HandleResult::Consumed)
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<HandleResult> {
        if !self.contains(req.window) {
            return Ok(HandleResult::Ignored);
        }

        let aux = ConfigureWindowAux::from_configure_request(&req);
        self.ctx.conn.configure_window(req.window, &aux)?;
        Ok(HandleResult::Consumed)
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<HandleResult> {
        if !self.contains(notif.window) {
            return Ok(HandleResult::Ignored);
        }

        if Some(notif.window) == self.frame {
            // FIXME
            let aux = ConfigureWindowAux::new()
                .x(0)
                .y(16)
                .width(notif.width as u32)
                .height((notif.height - 16) as u32);
            self.ctx.conn.configure_window(self.inner, &aux)?;
        }
        Ok(HandleResult::Consumed)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let root = self.ctx.root;
        if let Ok(void) = self.ctx.conn.reparent_window(self.inner, root, 0, 0) {
            void.ignore_error();
        }

        if let Some(frame) = self.frame {
            if let Ok(void) = self.ctx.conn.destroy_window(frame) {
                void.ignore_error();
            }
        }
    }
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Window {{ inner: {}, frame: {:?}, state: {:?} }}",
            self.inner, self.frame, self.state
        )
    }
}
