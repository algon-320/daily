use log::debug;
use std::collections::BTreeMap;

use crate::context::Context;
use crate::error::Result;
use crate::event::{EventHandlerMethods, HandleResult};
use crate::layout::{HorizontalLayout, Layout};

use x11rb::connection::Connection;
use x11rb::protocol::{randr::MonitorInfo, xproto::*};
use Window as Wid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Created,
    Mapped,
    Unmapped,
    Hidden,
}

fn is_mapped<'a>(&(_, &state): &(&'a Wid, &'a WindowState)) -> bool {
    state == WindowState::Mapped
}

#[derive()]
pub struct Screen {
    ctx: Context,
    pub id: usize,
    monitor: Option<MonitorInfo>,
    wins: BTreeMap<Wid, WindowState>,
    background: Wid, // background window
    layout: Box<dyn Layout>,
    border_visible: bool,
}

impl Screen {
    pub fn new(ctx: Context, id: usize) -> Result<Self> {
        let background = {
            let wid = ctx.conn.generate_id()?;
            let aux = CreateWindowAux::new()
                .background_pixel(0x148231)
                .event_mask(EventMask::FOCUS_CHANGE)
                .override_redirect(1); // special window
            ctx.conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                wid,
                ctx.root,
                0,  // x
                0,  // y
                16, // w
                16, // h
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )?;
            wid
        };

        let layout = HorizontalLayout::new(ctx.clone());

        Ok(Self {
            ctx,
            id,
            monitor: None,
            wins: BTreeMap::new(),
            background,
            layout: Box::new(layout),
            border_visible: false,
        })
    }

    pub fn attach(&mut self, monitor: MonitorInfo) -> Result<()> {
        let aux = ConfigureWindowAux::new()
            .x(monitor.x as i32)
            .y(monitor.y as i32)
            .width(monitor.width as u32)
            .height(monitor.height as u32);
        self.ctx.conn.configure_window(self.background, &aux)?;

        self.ctx.conn.map_window(self.background)?;
        for (&win, state) in self.wins.iter() {
            match state {
                WindowState::Mapped | WindowState::Hidden => {
                    self.ctx.conn.map_window(win)?;
                }
                _ => {}
            }
        }

        self.monitor = Some(monitor);
        self.refresh_layout()?;
        Ok(())
    }

    pub fn detach(&mut self) -> Result<()> {
        self.monitor = None;

        self.ctx.conn.unmap_window(self.background)?;
        for (&win, state) in self.wins.iter_mut() {
            if *state == WindowState::Mapped {
                *state = WindowState::Hidden;
                self.ctx.conn.unmap_window(win)?;
            }
        }

        Ok(())
    }

    pub fn add_window(&mut self, wid: Wid, state: WindowState) -> Result<()> {
        debug!("add_window: wid={}, state={:?}", wid, state);
        let state = if self.monitor.is_none() && state == WindowState::Mapped {
            WindowState::Hidden
        } else {
            state
        };

        self.wins.insert(wid, state);

        if state == WindowState::Mapped {
            self.refresh_layout()?;
            self.ctx.conn.map_window(wid)?;
        }
        Ok(())
    }

    pub fn forget_window(&mut self, wid: Wid) -> Result<WindowState> {
        let state = self.wins.remove(&wid).expect("unknown window");

        if state == WindowState::Mapped {
            self.ctx.conn.unmap_window(wid)?;
        }

        self.refresh_layout()?;
        Ok(state)
    }

    pub fn refresh_layout(&mut self) -> Result<()> {
        if self.monitor.is_none() {
            return Ok(());
        }

        let mut wins: Vec<Wid> = self
            .wins
            .iter()
            .filter(is_mapped)
            .map(|(wid, _)| wid)
            .copied()
            .collect();
        wins.sort();

        let mon = self.monitor.as_ref().unwrap();
        self.layout.layout(mon, &wins, self.border_visible)?;
        Ok(())
    }

    pub fn contains(&self, wid: Wid) -> Option<WindowState> {
        if wid == self.background {
            Some(WindowState::Mapped)
        } else {
            self.wins.get(&wid).copied().map(WindowState::from)
        }
    }

    pub fn focus_any(&mut self) -> Result<()> {
        debug!("screen {}: focus_any", self.id);
        let win = match self
            .wins
            .iter()
            .find(|(_, &state)| state == WindowState::Mapped || state == WindowState::Hidden)
            .map(|(wid, _)| wid)
        {
            Some(&first) => first,
            None => {
                debug!("screen {}: focus background", self.id);
                self.background
            }
        };
        self.ctx.focus_window(win)?;
        Ok(())
    }

    pub fn focus_next(&mut self) -> Result<()> {
        let old = self.ctx.get_focused_window()?;
        if !self.wins.contains_key(&old) {
            return self.focus_any();
        }

        let next = self
            .wins
            .iter()
            .filter(is_mapped)
            .map(|(wid, _)| wid)
            .copied()
            .cycle()
            .skip_while(|&w| w != old)
            .nth(1);

        if let Some(next) = next {
            debug!("focus_next: next={}", next);
            self.ctx.focus_window(next)?;
        }
        Ok(())
    }

    pub fn show_border(&mut self) {
        self.border_visible = true;
    }
    pub fn hide_border(&mut self) {
        self.border_visible = false;
    }
}

impl EventHandlerMethods for Screen {
    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if self.contains(wid).is_none() {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            return Ok(HandleResult::Consumed);
        }

        // Newly mapped window
        if let WindowState::Created = self.wins[&wid] {
            debug!("focus newly mapped window: wid={}", wid);
            self.ctx.focus_window(wid)?;
        }

        self.wins.insert(wid, WindowState::Mapped); // update
        self.refresh_layout()?;

        Ok(HandleResult::Consumed)
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;

        if self.contains(wid).is_none() {
            return Ok(HandleResult::Ignored);
        }

        if wid == self.background {
            return Ok(HandleResult::Consumed);
        }

        if let WindowState::Mapped = self.wins[&wid] {
            self.wins.insert(wid, WindowState::Unmapped);
        }
        self.refresh_layout()?;

        Ok(HandleResult::Consumed)
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        match self.contains(wid) {
            Some(..) => {
                self.wins.remove(&wid);
                Ok(HandleResult::Consumed)
            }
            None => Ok(HandleResult::Ignored),
        }
    }

    fn on_focus_in(&mut self, _focus_in: FocusInEvent) -> Result<HandleResult> {
        Ok(HandleResult::Consumed)
    }

    fn on_focus_out(&mut self, _focus_out: FocusInEvent) -> Result<HandleResult> {
        Ok(HandleResult::Consumed)
    }
}
