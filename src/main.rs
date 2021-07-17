mod config;
mod error;
mod event;
mod layout;
mod screen;
mod winman;

use log::{debug, error, info};
use serde::Deserialize;
use std::rc::Rc;

use crate::config::Config;
use error::{Error, Result};
use event::EventHandler;
use winman::WinMan;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::Window as Wid;
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize)]
pub enum Command {
    Quit,
    ShowBorder,
    HideBorder,
    Close,
    FocusNext,
    FocusPrev,
    OpenLauncher,
}

#[derive(Debug, Clone)]
pub struct Context {
    conn: Rc<RustConnection>,
    config: Rc<Config>,
    root: Wid,
}

impl Context {
    pub fn new(conn: RustConnection, config: Config, root: Wid) -> Self {
        Self {
            conn: Rc::new(conn),
            config: Rc::new(config),
            root,
        }
    }
}

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    let config = Config::load()?;

    // Connect with the X server (specified by $DISPLAY).
    let (conn, _) =
        RustConnection::connect(display_name.into()).map_err(|_| Error::ConnectionFailed)?;

    // Get a root window on the first screen.
    let screen = conn.setup().roots.get(0).expect("No screen");
    let root = screen.root;
    debug!("root = {}", root);

    let ctx = Context::new(conn, config, root);
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
