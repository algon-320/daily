use log::debug;
use std::rc::Rc;

use crate::config::Config;
use crate::error::{Error, Result};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, InputFocus, Window as Wid};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone)]
pub struct Context {
    pub conn: Rc<RustConnection>,
    pub config: Rc<Config>,
    pub root: Wid,
}

impl Context {
    pub fn new<S>(display_name: S) -> Result<Self>
    where
        S: Into<Option<&'static str>>,
    {
        let config = Config::load()?;

        // Connect with the X server
        let conn = RustConnection::connect(display_name.into())
            .map_err(|_| Error::ConnectionFailed)?
            .0;

        // Get a root window on the first screen.
        let screen = conn.setup().roots.get(0).ok_or(Error::NoScreen)?;
        let root = screen.root;
        debug!("root = {}", root);

        Ok(Self {
            conn: Rc::new(conn),
            config: Rc::new(config),
            root,
        })
    }

    pub fn focus_window(&self, win: Wid) -> Result<()> {
        debug!("set_input_focus --> {}", win);
        self.conn
            .set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME)?;
        Ok(())
    }

    pub fn get_focused_window(&self) -> Result<Wid> {
        Ok(self.conn.get_input_focus()?.reply()?.focus)
    }
}
