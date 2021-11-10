mod atom;
mod config;
mod context;
mod error;
mod event;
mod layout;
mod screen;
mod window;
mod winman;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum Command {
    Quit,
    Restart,
    ShowBorder,
    HideBorder,
    Close,
    Sink,
    FocusNext,
    FocusPrev,
    FocusNextMonitor,
    FocusPrevMonitor,
    NextLayout,
    Spawn(String),
    Screen(usize),
    MoveToScreen(usize),
    MovePointerRel(i16, i16), // (dx, dy)
    MouseClickLeft,
}

use error::{Error, Result};
use log::{error, info};

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    use event::EventHandler;
    use x11rb::connection::Connection;

    let ctx = context::init(display_name)?;
    let mut wm = winman::WinMan::new(ctx.clone())?;
    ctx.conn.flush()?;

    loop {
        let x11_event = ctx.conn.wait_for_event()?;
        wm.handle_event(x11_event)?;
        ctx.conn.flush()?;
    }
}

fn main() {
    env_logger::init();

    use std::process::exit;

    info!("hello");
    let status = match start(None) {
        Ok(()) | Err(Error::Quit) => {
            info!("goodbye");
            0
        }
        Err(Error::Restart) => {
            info!("try to restart");
            2
        }
        Err(err) => {
            error!("{}", err);
            1
        }
    };
    exit(status);
}
