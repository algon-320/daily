mod config;
mod context;
mod error;
mod event;
mod keybind;
mod layout;
mod screen;
mod winman;

use log::{error, info};

use context::Context;
use error::{Error, Result};
use event::EventHandler;
use winman::WinMan;

use x11rb::connection::Connection;

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    let ctx = Context::new(display_name)?;
    let mut wm = WinMan::new(ctx.clone())?;
    loop {
        let x11_event = ctx.conn.wait_for_event()?;
        wm.handle_event(x11_event.clone())?;
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
