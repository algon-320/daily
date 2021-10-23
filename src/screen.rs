use std::collections::HashSet;

use crate::context::Context;
use crate::error::Result;
use crate::event::{EventHandlerMethods, HandleResult};
use crate::layout::{HorizontalLayout, Layout};

use x11rb::connection::Connection;
use x11rb::protocol::{randr::MonitorInfo, xproto::*};
use Window as Wid;

#[derive(Debug)]
pub enum WindowState {
    Mapped,
    Unmapped,
}

#[derive()]
pub struct Screen {
    ctx: Context,
    pub id: usize,
    monitor: Option<MonitorInfo>,
    u_wins: HashSet<Wid>, // unmapped windows
    m_wins: HashSet<Wid>, // mapped windows
    layout: Box<dyn Layout>,
    border_visible: bool,
}

impl Screen {
    pub fn new(ctx: Context, id: usize, monitor: MonitorInfo) -> Self {
        let layout = Box::new(HorizontalLayout::new(ctx.clone()));
        Self {
            id,
            ctx,
            u_wins: HashSet::new(),
            m_wins: HashSet::new(),
            monitor: Some(monitor),
            layout,
            border_visible: false,
        }
    }

    pub fn add_window(&mut self, wid: Wid, state: WindowState) {
        match state {
            WindowState::Mapped => {
                self.m_wins.insert(wid);
            }
            WindowState::Unmapped => {
                self.u_wins.insert(wid);
            }
        }
    }

    pub fn refresh_layout(&mut self) -> Result<()> {
        // FIXME
        let wins: Vec<Wid> = self.m_wins.iter().copied().collect();

        let mon = self.monitor.as_ref().expect("screen not visible");
        self.layout.layout(mon, &wins, self.border_visible)?;
        Ok(())
    }

    pub fn contains(&self, wid: Wid) -> Option<WindowState> {
        if self.m_wins.contains(&wid) {
            Some(WindowState::Mapped)
        } else if self.u_wins.contains(&wid) {
            Some(WindowState::Unmapped)
        } else {
            None
        }
    }

    // FIXME: explicitly retains the focused window id
    pub fn focus_any(&mut self) -> Result<()> {
        if let Some(&first) = self.m_wins.iter().next() {
            self.ctx
                .conn
                .set_input_focus(InputFocus::POINTER_ROOT, first, x11rb::CURRENT_TIME)?;
            self.ctx.conn.flush()?;
        }
        Ok(())
    }

    pub fn focus_next(&mut self) -> Result<()> {
        let old = self.ctx.get_focused_window()?;
        let new = if self.m_wins.contains(&old) {
            self.m_wins
                .iter()
                .copied()
                .cycle()
                .skip_while(|&w| w != old)
                .nth(1)
        } else {
            return self.focus_any();
        };

        if let Some(new) = new {
            self.ctx
                .conn
                .set_input_focus(InputFocus::POINTER_ROOT, new, x11rb::CURRENT_TIME)?;
            self.ctx.conn.flush()?;
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
        match self.contains(wid) {
            Some(WindowState::Mapped) => Ok(HandleResult::Consumed),
            Some(WindowState::Unmapped) => {
                self.u_wins.remove(&wid);
                self.m_wins.insert(wid);
                self.refresh_layout()?;
                Ok(HandleResult::Consumed)
            }
            None => Ok(HandleResult::Ignored),
        }
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        match self.contains(wid) {
            Some(WindowState::Unmapped) => Ok(HandleResult::Consumed),
            Some(WindowState::Mapped) => {
                self.m_wins.remove(&wid);
                self.u_wins.insert(wid);
                self.refresh_layout()?;
                Ok(HandleResult::Consumed)
            }
            None => Ok(HandleResult::Ignored),
        }
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        match self.contains(wid) {
            Some(WindowState::Unmapped) | Some(WindowState::Mapped) => {
                self.u_wins.remove(&wid);
                self.m_wins.remove(&wid);
                Ok(HandleResult::Consumed)
            }
            None => Ok(HandleResult::Ignored),
        }
    }
}
