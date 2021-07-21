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
    pub monitor: Option<MonitorInfo>,
    u_wins: HashSet<Wid>,
    m_wins: HashSet<Wid>,
    layout: Box<dyn Layout>,
}

impl Screen {
    fn new_(ctx: Context, monitor: Option<MonitorInfo>) -> Self {
        let layout = Box::new(HorizontalLayout::new(ctx.clone()));
        Self {
            ctx,
            u_wins: HashSet::new(),
            m_wins: HashSet::new(),
            monitor,
            layout,
        }
    }

    pub fn new(ctx: Context) -> Self {
        Self::new_(ctx, None)
    }

    pub fn with_monitor(ctx: Context, monitor: MonitorInfo) -> Self {
        Self::new_(ctx, Some(monitor))
    }

    pub fn show(&mut self, monitor: MonitorInfo) {
        self.monitor = Some(monitor);
    }
    pub fn hide(&mut self) {
        self.monitor = None;
    }

    pub fn add_window(&mut self, wid: Wid, mapped: bool) {
        if mapped {
            self.m_wins.insert(wid);
        } else {
            self.u_wins.insert(wid);
        }
    }

    pub fn update_layout(&mut self) -> Result<()> {
        // FIXME
        let wins: Vec<Wid> = self.m_wins.iter().copied().collect();

        let mon = self.monitor.as_ref().expect("screen not visible");
        self.layout.layout(mon, &wins)?;
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
            self.m_wins.iter().copied().next()
        };

        if let Some(new) = new {
            self.ctx
                .conn
                .set_input_focus(InputFocus::POINTER_ROOT, new, x11rb::CURRENT_TIME)?;
            self.ctx.conn.flush()?;
        }
        Ok(())
    }
}

impl EventHandlerMethods for Screen {
    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.u_wins.contains(&wid) {
            self.u_wins.remove(&wid);
            self.m_wins.insert(wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.m_wins.contains(&wid) {
            self.m_wins.remove(&wid);
            self.u_wins.insert(wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        let wid = notif.window;
        if self.m_wins.contains(&wid) {
            self.m_wins.remove(&wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else if self.u_wins.contains(&wid) {
            self.u_wins.remove(&wid);
            self.update_layout()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }
}
