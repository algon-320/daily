use log::{debug, warn};

use x11rb::protocol::xproto::{Window as Wid, *};

use crate::context::Context;
use crate::error::Result;
use crate::event::EventHandlerMethods;

fn frame_window(ctx: &Context, wid: Wid, border_width: u32) -> Result<Wid> {
    use x11rb::connection::Connection as _;

    let geo = ctx.conn.get_geometry(wid)?.reply()?;

    let frame = {
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
            border_width as u16,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        )?;

        // WM_STATE
        let wm_state = ctx.conn.intern_atom(false, b"WM_STATE")?.reply()?.atom;
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_ne_bytes());
        data.extend_from_slice(&x11rb::NONE.to_ne_bytes());
        ctx.conn
            .change_property(PropMode::REPLACE, wid, wm_state, wm_state, 32, 2, &data)?;

        frame
    };
    ctx.conn.reparent_window(wid, frame, 0, 0)?;
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
    frame: Wid,
    inner: Wid,
    state: WindowState,
    ignore_unmap: usize,
    float_geometry: Option<Rectangle>,
    frame_visible: bool,
    highlighted: bool,
    border_width: u32,
}

impl Window {
    pub fn new(ctx: Context, inner: Wid, state: WindowState, border_width: u32) -> Result<Self> {
        let frame = frame_window(&ctx, inner, border_width)?;

        let mut ignore_unmap = 0;
        if state == WindowState::Mapped {
            // NOTE: ReparentWindow request automatically unmap `inner`.
            ignore_unmap += 1;

            ctx.conn.map_window(frame)?;
            ctx.conn.map_window(inner)?;
        }

        Ok(Self {
            ctx,
            frame,
            inner,
            state,
            ignore_unmap,
            float_geometry: None,
            frame_visible: false,
            highlighted: false,
            border_width,
        })
    }

    fn add_frame(&mut self) -> Result<()> {
        if self.frame_visible {
            warn!("the window is already framed");
            return Ok(());
        }

        let aux = ConfigureWindowAux::new()
            .x(0)
            .y(16) // FIXME
            .border_width(0);
        self.ctx.conn.configure_window(self.inner, &aux)?;

        let aux = ConfigureWindowAux::new().border_width(self.border_width as u32);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        self.frame_visible = true;
        self.update_ornament()?;
        Ok(())
    }

    fn remove_frame(&mut self) -> Result<()> {
        if !self.frame_visible {
            warn!("the window is not framed");
            return Ok(());
        }

        let aux = ConfigureWindowAux::new().x(0).y(0).border_width(0);
        self.ctx.conn.configure_window(self.inner, &aux)?;

        let aux = ConfigureWindowAux::new().border_width(self.border_width as u32);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        self.frame_visible = false;
        self.update_ornament()?;
        Ok(())
    }

    pub fn is_mapped(&self) -> bool {
        self.state == WindowState::Mapped
    }

    pub fn state(&self) -> WindowState {
        self.state
    }

    pub fn frame(&self) -> Wid {
        self.frame
    }
    pub fn inner(&self) -> Wid {
        self.inner
    }

    pub fn contains(&self, wid: Wid) -> bool {
        self.inner == wid || self.frame == wid
    }

    pub fn float(&mut self, rect: Rectangle) -> Result<()> {
        self.add_frame()?;

        // put this window at the top of window stack
        let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        self.float_geometry = Some(rect);
        Ok(())
    }
    pub fn sink(&mut self) -> Result<()> {
        self.remove_frame()?;

        self.float_geometry = None;
        Ok(())
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
            self.ctx.conn.map_window(self.frame)?;
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
            let frame = self.frame;
            if let Ok(attr) = self.ctx.conn.get_window_attributes(frame)?.reply() {
                if attr.map_state != MapState::UNMAPPED {
                    self.ctx.conn.unmap_window(frame)?;
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
            let wid = self.frame;
            if let Ok(attr) = self.ctx.conn.get_window_attributes(wid)?.reply() {
                if attr.map_state != MapState::UNMAPPED {
                    self.ctx.conn.unmap_window(wid)?;
                    self.ignore_unmap += 1;
                }
            }
            self.state = WindowState::Hidden;
        }
        Ok(())
    }

    pub fn focus(&mut self) -> Result<()> {
        self.ctx.focus_window(self.inner)
    }

    fn draw_frame(&mut self) -> Result<()> {
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
        self.ctx.conn.poly_fill_rectangle(self.frame, gc, &[rect])?;
        Ok(())
    }

    fn update_ornament(&mut self) -> Result<()> {
        if self.frame_visible {
            self.draw_frame()?;
        }

        let border = self.ctx.config.border;
        let color = if self.highlighted {
            border.color_focused
        } else {
            border.color_regular
        };
        let aux = ChangeWindowAttributesAux::new().border_pixel(color);
        self.ctx.conn.change_window_attributes(self.frame, &aux)?;
        Ok(())
    }

    pub fn set_highlight(&mut self, highlight: bool) -> Result<()> {
        self.highlighted = highlight;
        self.update_ornament()?;
        Ok(())
    }

    pub fn close(self) {
        if let Ok(void) = self.ctx.conn.destroy_window(self.inner) {
            let _ = void.check();
        }
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

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<()> {
        if !self.contains(notif.window) {
            return Ok(());
        }

        // Ignore the event if it is caused by us.
        // FIXME: This kind of filtering should be done by checking the sequence number
        //        corresponding to the causing request.
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

        assert!(req.window == self.inner);

        let aux = ConfigureWindowAux::new().border_width(req.border_width as u32);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        let aux = if self.frame_visible {
            let height = 16;
            ConfigureWindowAux::from_configure_request(&req)
                .x(0)
                .y(height as i32)
                .border_width(0)
                .height((req.height - height) as u32)
        } else {
            ConfigureWindowAux::from_configure_request(&req)
                .x(0)
                .y(0)
                .border_width(0)
        };
        self.ctx.conn.configure_window(self.inner, &aux)?;

        Ok(())
    }

    fn on_configure_notify(&mut self, notif: ConfigureNotifyEvent) -> Result<()> {
        if !self.contains(notif.window) {
            return Ok(());
        }

        if notif.window == self.frame {
            // Ensure delivery of ConfigureNotify at the following `configure_window`
            // NOTE: Because ConfigureWindow request which doesn't change the current configuration
            //       will not generate any ConfigureNotify on the window,
            //       we make a extra request here to ensure that.
            let aux = ConfigureWindowAux::new().x(1);
            self.ctx.conn.configure_window(self.inner, &aux)?;

            let aux = if self.frame_visible {
                let height = 16;
                ConfigureWindowAux::new()
                    .x(0)
                    .y(height as i32)
                    .border_width(0)
                    .width(notif.width as u32)
                    .height((notif.height - height) as u32)
            } else {
                ConfigureWindowAux::new()
                    .x(0)
                    .y(0)
                    .border_width(0)
                    .width(notif.width as u32)
                    .height(notif.height as u32)
            };
            self.ctx.conn.configure_window(self.inner, &aux)?;
        }
        Ok(())
    }

    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        if self.frame == ev.window {
            self.draw_frame()?;
        }
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        debug!("Window drop");

        let root = self.ctx.root;
        if let Ok(void) = self.ctx.conn.reparent_window(self.inner, root, 0, 0) {
            let _ = void.check();
        }

        if let Ok(void) = self.ctx.conn.destroy_window(self.frame) {
            let _ = void.check();
        }
    }
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Window {{ inner: {:08X}, frame: {:08X}, state: {:?} }}",
            self.inner, self.frame, self.state
        )
    }
}
