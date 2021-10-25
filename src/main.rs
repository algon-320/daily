mod config;
mod context;
mod error;
mod event;
mod layout;
mod screen;
mod winman;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum Command {
    Quit,
    ShowBorder,
    HideBorder,
    Close,
    FocusNext,
    FocusPrev,
    FocusNextMonitor,
    FocusPrevMonitor,
    OpenLauncher,
    OpenTerminal,
    Screen1,
    Screen2,
    Screen3,
    Screen4,
    Screen5,
    MoveToScreen1,
    MoveToScreen2,
    MoveToScreen3,
    MoveToScreen4,
    MoveToScreen5,
}

use error::{Error, Result};
use log::{error, info};

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    use event::EventHandler;
    use x11rb::connection::Connection;

    let ctx = context::Context::new(display_name)?;
    let mut wm = winman::WinMan::new(ctx.clone())?;
    loop {
        let x11_event = ctx.conn.wait_for_event()?;
        wm.handle_event(x11_event)?;
    }
}

fn main() {
    env_logger::init();

    info!("hello");
    match start(None) {
        Ok(()) | Err(Error::Quit) => {
            info!("goodbye");
        }
        Err(err) => {
            error!("{}", err);
        }
    }
}
