use log::debug;

use x11rb::protocol::xproto::{Window as Wid, *};

use crate::context::Context;
use crate::error::Result;
use crate::event::EventHandlerMethods;

fn get_wm_protocols(ctx: &Context, wid: Wid) -> Result<Vec<Atom>> {
    // NOTE: https://www.x.org/releases/X11R7.7/doc/xorg-docs/icccm/icccm.html#WM_PROTOCOLS_Property

    let wm_protocols = ctx.atom.WM_PROTOCOLS;
    let res = ctx
        .conn
        .get_property(false, wid, wm_protocols, AtomEnum::ATOM, 0, std::u32::MAX)?
        .reply()?;

    if res.type_ == x11rb::NONE || res.value.len() % 4 != 0 {
        return Ok(Vec::new());
    }

    let protocols = res
        .value32()
        .map(|iter| iter.collect())
        .unwrap_or_else(|| Vec::new());

    Ok(protocols)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Created,
    Mapped,
    Unmapped,
}

#[derive()]
pub struct Window {
    ctx: Context,
    frame: Wid,
    inner: Wid,
    state: WindowState,
    hidden: bool,
    float_geometry: Option<Rectangle>,
    frame_visible: bool,
    highlighted: bool,
    border_width: u32,
    gc: Gcontext,
    is_wm_delete_compliant: bool,
}

impl Window {
    pub fn new(ctx: Context, inner: Wid, state: WindowState, border_width: u32) -> Result<Self> {
        use x11rb::connection::Connection as _;

        let mut is_wm_delete_compliant = false;

        // Examine WM_PROTOCOLS
        {
            let wm_protocols = get_wm_protocols(&ctx, inner)?;

            debug!("WM_PROTOCOLS of {:08X}: {:?}", inner, wm_protocols);
            for proto in wm_protocols {
                let name_bytes = ctx.conn.get_atom_name(proto)?.reply()?.name;
                let name =
                    String::from_utf8(name_bytes).unwrap_or_else(|e| format!("{:?}", e.as_bytes()));
                debug!("WM_PROTOCOLS: {}", name);

                if proto == ctx.atom.WM_DELETE_WINDOW {
                    is_wm_delete_compliant = true;
                }
            }
        }

        // Reparent
        let geo = ctx.conn.get_geometry(inner)?.reply()?;
        let frame = {
            let frame = ctx.conn.generate_id()?;
            let mask = EventMask::SUBSTRUCTURE_NOTIFY
                | EventMask::SUBSTRUCTURE_REDIRECT
                | EventMask::EXPOSURE;
            let aux = CreateWindowAux::new().event_mask(mask).override_redirect(1);
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
            let wm_state = ctx.atom.WM_STATE;
            let mut data = Vec::new();
            data.extend_from_slice(&1u32.to_ne_bytes());
            data.extend_from_slice(&x11rb::NONE.to_ne_bytes());
            ctx.conn
                .change_property(PropMode::REPLACE, inner, wm_state, wm_state, 32, 2, &data)?;

            ctx.conn.reparent_window(inner, frame, 0, 0)?;

            frame
        };

        if state == WindowState::Mapped {
            ctx.conn.map_window(frame)?;
            ctx.conn.map_window(inner)?;
        }

        let gc = ctx.conn.generate_id()?;
        {
            let font = ctx.conn.generate_id()?;
            ctx.conn.open_font(font, b"fixed")?.check()?;

            let aux = CreateGCAux::new().font(font);
            ctx.conn.create_gc(gc, frame, &aux)?;

            ctx.conn.close_font(font)?;
        }

        Ok(Self {
            ctx,
            frame,
            inner,
            state,
            hidden: false,
            float_geometry: None,
            frame_visible: false,
            highlighted: false,
            border_width,
            gc,
            is_wm_delete_compliant,
        })
    }

    pub fn net_wm_type(&self) -> Result<Option<Atom>> {
        let net_wm_type = self.ctx.atom._NET_WM_WINDOW_TYPE;
        let value = self
            .ctx
            .conn
            .get_property(false, self.inner, net_wm_type, AtomEnum::ATOM, 0, 1)?
            .reply()?
            .value;
        if value.len() < 4 {
            return Ok(None);
        }

        Ok(value[..].try_into().map(Atom::from_ne_bytes).ok())
    }

    pub fn close(self) -> Result<()> {
        if self.is_wm_delete_compliant {
            debug!("send WM_DELETE_WINDOW to {:08X}", self.inner);

            // NOTE: https://www.x.org/releases/X11R7.7/doc/xorg-docs/icccm/icccm.html#ClientMessage_Events
            let wm_protocols = self.ctx.atom.WM_PROTOCOLS;
            let wm_delete_window = self.ctx.atom.WM_DELETE_WINDOW;
            let data = ClientMessageData::from([wm_delete_window, x11rb::CURRENT_TIME, 0, 0, 0]);
            let event = ClientMessageEvent::new(32, self.inner, wm_protocols, data);
            self.ctx.conn.send_event(false, self.inner, 0_u32, event)?;
        } else {
            debug!("destroy window {:08X}", self.inner);
            self.ctx.conn.destroy_window(self.inner)?.check()?;
        }
        Ok(())
    }

    pub fn is_mapped(&self) -> bool {
        self.state == WindowState::Mapped
    }

    pub fn is_viewable(&self) -> bool {
        !self.hidden && self.state == WindowState::Mapped
    }

    pub fn frame(&self) -> Wid {
        self.frame
    }

    pub fn contains(&self, wid: Wid) -> bool {
        self.inner == wid || self.frame == wid
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
        if !self.hidden {
            self.ctx.conn.map_window(self.frame)?;
            self.ctx.conn.map_window(self.inner)?;
        }

        // Focus this window if it's a newly mapped one
        if self.state == WindowState::Created {
            debug!("focus newly mapped window: win={:?}", self);
            self.focus()?;
        }
        self.state = WindowState::Mapped;
        Ok(())
    }

    pub fn unmap(&mut self) -> Result<()> {
        self.state = WindowState::Unmapped;
        self.ctx.conn.unmap_window(self.frame)?;
        Ok(())
    }

    /// Map the window without changing its state.
    pub fn show(&mut self) -> Result<()> {
        assert!(self.hidden);
        self.hidden = false;
        self.ctx.conn.map_window(self.frame)?;
        Ok(())
    }

    /// Unmap the window without changing its state.
    pub fn hide(&mut self) -> Result<()> {
        assert!(!self.hidden);
        self.hidden = true;
        self.ctx.conn.unmap_window(self.frame)?;
        Ok(())
    }

    pub fn focus(&mut self) -> Result<()> {
        self.ctx.focus_window(self.inner)
    }

    pub fn float(&mut self, mut rect: Rectangle) -> Result<()> {
        self.add_frame()?;

        // put this window at the top of window stack
        let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        // add space for the frame
        rect.height += 16; // FIXME

        self.float_geometry = Some(rect);
        Ok(())
    }

    pub fn sink(&mut self) -> Result<()> {
        self.remove_frame()?;

        self.float_geometry = None;
        Ok(())
    }

    pub fn set_highlight(&mut self, highlight: bool) -> Result<()> {
        self.highlighted = highlight;
        self.update_ornament()?;
        Ok(())
    }

    pub fn configure(&self, aux: &ConfigureWindowAux) -> Result<()> {
        // Use self.border_width if border_width is not specified.
        let bw = aux.border_width.unwrap_or(self.border_width);
        let aux = aux.border_width(bw);
        self.ctx.conn.configure_window(self.frame, &aux)?;

        // dummy request
        // Ensure delivery of ConfigureNotify at the following `configure_window`
        // NOTE: Because ConfigureWindow request which doesn't change the current configuration
        let dummy_aux = ConfigureWindowAux::new().x(1);
        self.ctx.conn.configure_window(self.inner, &dummy_aux)?;

        let mut inner_aux = ConfigureWindowAux::new()
            .x(0)
            .y(if self.frame_visible { 16 } else { 0 }) // FIXME
            .border_width(0);

        if let Some(w) = aux.width {
            inner_aux = inner_aux.width(w);
        }
        if let Some(h) = aux.height {
            let y = inner_aux.y.unwrap();
            inner_aux = inner_aux.height(h - y as u32);
        }

        self.ctx.conn.configure_window(self.inner, &inner_aux)?;
        Ok(())
    }

    fn add_frame(&mut self) -> Result<()> {
        if self.frame_visible {
            return Ok(());
        }

        self.frame_visible = true;
        let aux = ConfigureWindowAux::new();
        self.configure(&aux)?;

        self.update_ornament()?;
        Ok(())
    }

    fn remove_frame(&mut self) -> Result<()> {
        if !self.frame_visible {
            return Ok(());
        }

        self.frame_visible = false;
        let aux = ConfigureWindowAux::new();
        self.configure(&aux)?;

        self.update_ornament()?;
        Ok(())
    }

    fn draw_frame(&mut self) -> Result<()> {
        let conn = &self.ctx.conn;

        // Fetch window info
        let geo = conn.get_geometry(self.frame)?.reply()?;
        let reply = self
            .ctx
            .conn
            .get_property(
                false,
                self.inner,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                0,
                std::u32::MAX,
            )?
            .reply();
        let win_name = reply
            .map(|reply| reply.value)
            .unwrap_or_else(|_| b"(unknown)".to_vec());
        let win_name = String::from_utf8_lossy(&win_name);

        // Clear
        let color = if self.highlighted {
            self.ctx.config.border.color_focused
        } else {
            self.ctx.config.border.color_regular
        };
        let aux = ChangeGCAux::new().foreground(color).background(color);
        conn.change_gc(self.gc, &aux)?;
        conn.poly_fill_rectangle(
            self.frame,
            self.gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: geo.width,
                height: 16,
            }],
        )?;

        // Window ID and name
        let title = format!("0x{:07X} -- {}", self.inner, win_name);
        let title = title.as_bytes();
        let aux = ChangeGCAux::new().foreground(0xFFFFFF);
        conn.change_gc(self.gc, &aux)?;
        conn.image_text8(self.frame, self.gc, 4, 13, title)?;

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

        assert!(notif.window == self.inner);
        self.unmap()?;

        Ok(())
    }

    fn on_expose(&mut self, ev: ExposeEvent) -> Result<()> {
        assert!(ev.window == self.frame);
        if self.is_viewable() {
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
