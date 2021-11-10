use log::debug;
use std::rc::Rc;

use crate::config::Config;
use crate::error::{Error, Result};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, InputFocus, Window as Wid};
use x11rb::rust_connection::RustConnection;

pub type Context = Rc<ContextInner>;

pub fn init<S>(display_name: S) -> Result<Context>
where
    S: Into<Option<&'static str>>,
{
    let inner = ContextInner::new(display_name)?;
    Ok(Rc::new(inner))
}

#[derive(Debug)]
pub struct ContextInner {
    pub conn: RustConnection,
    pub config: Config,
    pub root: Wid,
}

impl ContextInner {
    fn new<S>(display_name: S) -> Result<Self>
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
        debug!("root = {:08X}", root);

        Ok(Self { conn, config, root })
    }

    pub fn focus_window(&self, win: Wid) -> Result<()> {
        debug!("set_input_focus --> {:08X}", win);
        self.conn
            .set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME)?;
        Ok(())
    }

    pub fn get_focused_window(&self) -> Result<Option<Wid>> {
        fn is_window(wid: Wid) -> bool {
            wid != InputFocus::POINTER_ROOT.into() && wid != InputFocus::NONE.into()
        }

        let focus = self.conn.get_input_focus()?.reply()?.focus;
        Ok(if is_window(focus) { Some(focus) } else { None })
    }
}
