use log::debug;

use x11rb::protocol::xproto::{Window as Wid, *};

use crate::context::Context;
use crate::error::Result;
use crate::event::EventHandlerMethods;

fn frame_window(ctx: &Context, wid: Wid) -> Result<Wid> {
    use x11rb::connection::Connection as _;

    let geo = ctx.conn.get_geometry(wid)?.reply()?;
    let frame = ctx.conn.generate_id()?;
    let mask =
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT | EventMask::EXPOSURE;
    let aux = CreateWindowAux::new().event_mask(mask);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Created,
    Mapped,
    Unmapped,
    Hidden,
}

#[derive()]
pub struct Window {
    ctx: Context,
    frame: Option<Wid>,
    inner: Wid,
    state: WindowState,
    ignore_unmap: usize,
    float_geometry: Option<Rectangle>,
    highlighted: bool,
}

impl Window {
    #[allow(unused)]
    pub fn new(ctx: Context, inner: Wid, state: WindowState) -> Result<Self> {
        if state == WindowState::Mapped {
            ctx.conn.map_window(inner)?;
        }

        Ok(Self {
            ctx,
            frame: None,
            inner,
            state,
            ignore_unmap: 0,
            float_geometry: None,
            highlighted: false,
        })
    }

    #[allow(unused)]
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
            ignore_unmap: 0,
            float_geometry: None,
            highlighted: false,
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

    pub fn float(&mut self, rect: Rectangle) -> Result<()> {
        let wid = self.id();

        // put this window at the top of window stack
        let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        self.ctx.conn.configure_window(wid, &aux)?;

        self.float_geometry = Some(rect);
        Ok(())
    }
    pub fn sink(&mut self) {
        self.float_geometry = None;
    }
    pub fn is_floating(&self) -> bool {
        self.float_geometry.is_some()
    }

    pub fn set_float_geometry(&mut self, rect: Rectangle) {
        assert!(self.is_floating());
        self.float_geometry = Some(rect);
    }
    pub fn get_float_geometry(&self) -> Option<Rectangle> {
        self.float_geometry
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
        if self.state == WindowState::Mapped {
            if let Some(frame) = self.frame {
                if let Ok(attr) = self.ctx.conn.get_window_attributes(frame)?.reply() {
                    if attr.map_state != MapState::UNMAPPED {
                        self.ctx.conn.unmap_window(frame)?;
                        // ignore the next unmap event
                        self.ignore_unmap += 1;
                    }
                }
            }

            // the reply() will return Err if the self.inner has been already destroyed.
            if let Ok(attr) = self.ctx.conn.get_window_attributes(self.inner)?.reply() {
                if attr.map_state != MapState::UNMAPPED {
                    self.ctx.conn.unmap_window(self.inner)?;

                    // ignore the next unmap event
                    self.ignore_unmap += 1;
                }
            }

            self.state = WindowState::Unmapped;
        }
        Ok(())
    }

    pub fn hide(&mut self) -> Result<()> {
        if self.state != WindowState::Hidden {
            let wid = self.frame.unwrap_or(self.inner);
            self.ctx.conn.unmap_window(wid)?;
            self.ignore_unmap += 1;

            self.state = WindowState::Hidden;
        }
        Ok(())
    }

    pub fn focus(&mut self) -> Result<()> {
        self.ctx.focus_window(self.inner)
    }

    fn draw_frame(&mut self) -> Result<()> {
        let frame = match self.frame {
            Some(f) => f,
            None => return Ok(()),
        };

        let gc = if self.highlighted {
            self.ctx.color_focused
        } else {
            self.ctx.color_regular
        };

        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 10000,
            height: 16,
        };
        self.ctx.conn.poly_fill_rectangle(frame, gc, &[rect])?;
        Ok(())
    }

    fn update_ornament(&mut self) -> Result<()> {
        if self.frame.is_some() {
            self.draw_frame()?;
        } else {
            let border = self.ctx.config.border;
            let color = if self.highlighted {
                border.color_focused
            } else {
                border.color_regular
            };
            let aux = ChangeWindowAttributesAux::new().border_pixel(color);
            self.ctx.conn.change_window_attributes(self.inner, &aux)?;
        }
        Ok(())
    }

    pub fn set_highlight(&mut self, highlight: bool) -> Result<()> {
        self.highlighted = highlight;
        self.update_ornament()?;
        Ok(())
    }
}

impl EventHandlerMethods for Window {
    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<()> {
        if !self.contains(req.window) {
            return Ok(());
        }

        self.map()?;
        Ok(())
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<()> {
        if !self.contains(notif.window) {
            return Ok(());
        }
        Ok(())
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<()> {
        if !self.contains(notif.window) {
            return Ok(());
        }

        // Ignore the event if it is caused by us.
        if self.ignore_unmap > 0 {
            self.ignore_unmap -= 1;
            return Ok(());
        }

        // For unmap events caused by another client, we have to do the following:
        // - update the state
        // - unmap the frame window if it exists
        self.unmap()?;

        Ok(())
    }

    fn on_configure_request(&mut self, req: ConfigureRequestEvent) -> Result<()> {
        if !self.contains(req.window) {
            return Ok(());
        }

        let aux = ConfigureWindowAux::from_configure_request(&req);
        self.ctx.conn.configure_window(req.window, &aux)?;
        Ok(())
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<()> {
        if !self.contains(notif.window) {
            return Ok(());
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
        Ok(())
    }

    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        if let Some(frame) = self.frame {
            if frame == ev.window {
                self.draw_frame()?;
            }
        }
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        debug!("Window drop");

        if let Some(frame) = self.frame {
            let root = self.ctx.root;
            if let Ok(void) = self.ctx.conn.reparent_window(self.inner, root, 0, 0) {
                void.ignore_error();
            }

            if let Ok(void) = self.ctx.conn.destroy_window(frame) {
                void.ignore_error();
            }
        }

        if let Ok(void) = self.ctx.conn.destroy_window(self.inner) {
            void.ignore_error();
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
