use log::debug;
use std::rc::Rc;

use crate::config::Config;
use crate::error::{Error, Result};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, Gcontext, InputFocus, Window as Wid,
};
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
    pub color_focused: Gcontext,
    pub color_regular: Gcontext,
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
        debug!("root = {}", root);

        let color_focused = conn.generate_id()?;
        let aux = CreateGCAux::new().foreground(config.border.color_focused);
        conn.create_gc(color_focused, root, &aux)?;
        debug!("color_focused gc = {}", color_focused);

        let color_regular = conn.generate_id()?;
        let aux = CreateGCAux::new().foreground(config.border.color_regular);
        conn.create_gc(color_regular, root, &aux)?;
        debug!("color_regular gc = {}", color_regular);

        Ok(Self {
            conn,
            config,
            root,
            color_focused,
            color_regular,
        })
    }

    pub fn focus_window(&self, win: Wid) -> Result<()> {
        debug!("set_input_focus --> {}", win);
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

impl Drop for ContextInner {
    fn drop(&mut self) {
        if let Ok(void) = self.conn.free_gc(self.color_focused) {
            void.ignore_error();
        }
        if let Ok(void) = self.conn.free_gc(self.color_regular) {
            void.ignore_error();
        }
    }
}
