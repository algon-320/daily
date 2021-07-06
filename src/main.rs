mod error;

use error::{Error, Result};
use log::{debug, error, info, trace, warn};

use std::rc::Rc;

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
    Event,
};
use Window as Wid;

#[derive(Debug)]
pub enum Command {
    NoOp,

    Close(Wid),
    FocusNext,
    FocusPrev,
    ShowBorder,
    HideBorder,

    MapRequest { window: Wid },
    MapNotify { window: Wid },
    UnmapNotify { window: Wid },
    CreateNotify { window: Wid },
    DestroyNotify { window: Wid },

    MousePress { window: Wid, x: i16, y: i16 },
    MouseRelease { window: Wid, x: i16, y: i16 },
    MouseMove,
}

use std::collections::HashMap;

#[derive(Debug)]
struct WindowState {
    mapped: bool,
}

#[derive(Debug)]
struct WinMan<C: Connection> {
    conn: Rc<C>,
    config: Rc<Config>,
    root: Wid,
    windows: HashMap<Wid, WindowState>,
    monitor_size: (u16, u16),
    border_visible: bool,
}

impl<C: Connection> WinMan<C> {
    pub fn new(conn: Rc<C>, config: Rc<Config>, root: Wid) -> Result<Self> {
        let mut wm = Self {
            conn,
            config,
            root,
            windows: HashMap::new(),
            monitor_size: (0, 0),
            border_visible: false,
        };
        wm.init()?;

        Ok(wm)
    }

    fn init(&mut self) -> Result<()> {
        self.refresh_monitor()?;

        // List all exising windows
        let preexist = self.conn.query_tree(self.root)?.reply()?.children;

        // Configure existing windows
        for wid in preexist {
            let attr = self.conn.get_window_attributes(wid)?.reply()?;
            let state = WindowState {
                mapped: attr.map_state == MapState::VIEWABLE,
            };
            self.windows.insert(wid, state);
        }

        debug!("windows = {:?}", self.windows.keys());
        self.refresh_layout_horizontal_split()?;

        Ok(())
    }

    fn refresh_monitor(&mut self) -> Result<()> {
        let monitors_reply = self.conn.randr_get_monitors(self.root, true)?.reply()?;
        let mon = monitors_reply.monitors.get(0).expect("no monitor");
        self.monitor_size = (mon.width, mon.height);
        Ok(())
    }

    fn refresh_layout_horizontal_split(&mut self) -> Result<()> {
        let mapped_count = self
            .windows
            .iter()
            .filter(|(_, state)| state.mapped)
            .count();
        if mapped_count == 0 {
            return Ok(());
        }

        let focused = self.conn.get_input_focus()?.reply()?.focus;
        debug!("focused = {}", focused);

        let w = (self.monitor_size.0 / mapped_count as u16) as u32;
        let h = self.monitor_size.1 as u32;
        let mut x = 0;

        for (&wid, _) in self.windows.iter().filter(|(_, state)| state.mapped) {
            let show_border = self.border_visible && wid == focused;
            let mut conf = ConfigureWindowAux::new().x(x).y(0);
            if show_border {
                const BORDER_WIDTH: u32 = 2;
                conf = conf
                    .border_width(BORDER_WIDTH)
                    .width(w - BORDER_WIDTH * 2)
                    .height(h - BORDER_WIDTH * 2);
            } else {
                conf = conf.border_width(0).width(w).height(h)
            }
            self.conn.configure_window(wid, &conf)?;

            if show_border {
                let attr = ChangeWindowAttributesAux::new().border_pixel(0xFF8882);
                self.conn.change_window_attributes(wid, &attr)?;
            }

            x += w as i32;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn map_window(&mut self, wid: Wid) -> Result<()> {
        let state = self.windows.get_mut(&wid).expect("unknown window");
        state.mapped = true;
        self.refresh_layout_horizontal_split()?;
        Ok(())
    }

    fn unmap_window(&mut self, wid: Wid) -> Result<()> {
        let state = self.windows.get_mut(&wid).expect("unknown window");
        state.mapped = false;
        self.refresh_layout_horizontal_split()?;
        Ok(())
    }

    fn focus_next(&mut self) -> Result<()> {
        let focused = self.conn.get_input_focus()?.reply()?.focus;
        let next = self
            .windows
            .iter()
            .filter(|(_, st)| st.mapped)
            .skip_while(|(&wid, _)| wid != focused)
            .nth(1)
            .or_else(|| self.windows.iter().find(|(_, st)| st.mapped))
            .map(|(wid, _)| wid);
        if let Some(&next) = next {
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, next, x11rb::CURRENT_TIME)?
                .check()?;
        }
        Ok(())
    }

    pub fn process(&mut self, cmd: Command) -> Result<()> {
        trace!("{:?}", cmd);
        match cmd {
            Command::NoOp => {}

            // Close focused window
            Command::Close(wid) => {
                self.focus_next()?;
                self.conn.destroy_window(wid)?;
                self.conn.flush()?;
            }

            // Change focus to the next
            Command::FocusNext => {
                self.focus_next()?;
                self.refresh_layout_horizontal_split()?;
            }
            // Change focus to the previous
            Command::FocusPrev => {}

            Command::ShowBorder => {
                self.border_visible = true;
                self.refresh_layout_horizontal_split()?;
            }
            Command::HideBorder => {
                self.border_visible = false;
                self.refresh_layout_horizontal_split()?;
            }

            // Change focus to the next
            Command::MapRequest { window } => {
                self.conn.map_window(window)?;
                self.conn.flush()?;
            }
            Command::MapNotify { window } => {
                self.map_window(window)?;

                self.conn
                    .set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME)?;
                self.conn.flush()?;
            }
            Command::UnmapNotify { window } => {
                if self.windows.contains_key(&window) {
                    self.unmap_window(window)?;
                }
            }
            Command::CreateNotify { window } => {
                let state = WindowState { mapped: false };
                self.windows.insert(window, state);
            }
            Command::DestroyNotify { window } => {
                if self.windows.contains_key(&window) {
                    self.windows.remove(&window);
                }
            }

            // Mouse events
            Command::MousePress { window, .. } => {
                debug!("set_input_focus");
                self.conn
                    .set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME)?;
                self.conn.flush()?;
            }
            Command::MouseRelease { .. } => {}
            Command::MouseMove => {}
        }
        Ok(())
    }
}

struct EventHandler<C: Connection> {
    conn: Rc<C>,
    config: Rc<Config>,
}

impl<C: Connection> EventHandler<C> {
    pub fn new(conn: Rc<C>, config: Rc<Config>, root: Wid) -> Result<Self> {
        // Become a window manager of the root window.
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let aux = ChangeWindowAttributesAux::new().event_mask(mask);
        conn.change_window_attributes(root, &aux)?
            .check()
            .map_err(|_| Error::WmAlreadyExists)?;

        // Grab keys
        for (modif, keycode) in config.bounded_keys() {
            conn.grab_key(true, root, modif, keycode, GrabMode::ASYNC, GrabMode::ASYNC)?
                .check()
                .map_err(|_| Error::KeyAlreadyGrabbed)?;
        }

        // Grab mouse buttons
        let event_mask: u32 = (EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE).into();
        conn.grab_button(
            false,
            root,
            event_mask as u16,
            GrabMode::SYNC,
            GrabMode::ASYNC,
            root,
            x11rb::NONE,
            ButtonIndex::M1,
            ModMask::ANY,
        )?
        .check()
        .map_err(|_| Error::ButtonAlreadyGrabbed)?;

        // Receive RROutputChangeNotifyEvent
        conn.randr_select_input(root, randr::NotifyMask::OUTPUT_CHANGE)?;

        Ok(Self { conn, config })
    }

    pub fn handle(&mut self, event: Event) -> Result<Command> {
        match event {
            Event::KeyPress(e) => {
                let op = self.config.get_keybind(e.state, e.detail);
                let op = op.as_deref();
                match op {
                    Some("super-press") => Ok(Command::ShowBorder),
                    Some("exit") => Err(Error::Quit),
                    Some("close") => {
                        let focused = self.conn.get_input_focus()?.reply()?.focus;
                        Ok(Command::Close(focused))
                    }
                    Some("focus-next") => Ok(Command::FocusNext),
                    Some("focus-prev") => Ok(Command::FocusPrev),
                    Some("open-launcher") => {
                        let _ = std::process::Command::new("/usr/bin/dmenu_run").spawn();
                        Ok(Command::NoOp)
                    }
                    Some(_) | None => {
                        warn!("unhandled KeyPress event");
                        Ok(Command::NoOp)
                    }
                }
            }
            Event::KeyRelease(e) => {
                let op = self.config.get_keybind(e.state, e.detail);
                match op.as_deref() {
                    Some("super-release") => Ok(Command::HideBorder),
                    Some(_) | None => Ok(Command::NoOp),
                }
            }

            Event::ButtonPress(e) => {
                self.conn
                    .allow_events(Allow::REPLAY_POINTER, x11rb::CURRENT_TIME)?
                    .check()?;
                if e.child == x11rb::NONE {
                    Ok(Command::NoOp)
                } else {
                    Ok(Command::MousePress {
                        window: e.child,
                        x: e.event_x,
                        y: e.event_y,
                    })
                }
            }
            Event::ButtonRelease(_) => Ok(Command::NoOp),

            Event::MapRequest(req) => Ok(Command::MapRequest { window: req.window }),
            Event::MapNotify(notif) => {
                if notif.override_redirect {
                    Ok(Command::NoOp)
                } else {
                    Ok(Command::MapNotify {
                        window: notif.window,
                    })
                }
            }
            Event::UnmapNotify(notif) => Ok(Command::UnmapNotify {
                window: notif.window,
            }),
            Event::CreateNotify(notif) => {
                if notif.override_redirect {
                    Ok(Command::NoOp)
                } else {
                    Ok(Command::CreateNotify {
                        window: notif.window,
                    })
                }
            }
            Event::DestroyNotify(notif) => Ok(Command::DestroyNotify {
                window: notif.window,
            }),

            event => {
                debug!("unhandled event: {:?}", event);
                Ok(Command::NoOp)
            }
        }
    }
}

#[repr(u8)]
enum KeyCode {
    Tab = 23,
    Q = 24,
    C = 54,
    P = 33,
    Super = 133,
}

use std::cell::RefCell;

#[derive(Debug)]
pub struct Config {
    keybind: RefCell<HashMap<(u16, u8), String>>,
}

impl Config {
    pub fn add_keybind<S>(&self, modifier: u16, keycode: u8, op: S)
    where
        S: Into<String>,
    {
        let mut keybind = self.keybind.borrow_mut();
        keybind.insert((modifier, keycode), op.into());
    }

    pub fn get_keybind(&self, modifier: u16, keycode: u8) -> Option<String> {
        let keybind = self.keybind.borrow();
        keybind.get(&(modifier, keycode)).cloned()
    }

    pub fn bounded_keys(&self) -> Vec<(u16, u8)> {
        let keybind = self.keybind.borrow();
        let mut keys = Vec::new();
        for &(m, c) in keybind.keys() {
            keys.push((m, c));
        }
        keys
    }
}

impl Default for Config {
    fn default() -> Self {
        let config = Config {
            keybind: Default::default(),
        };

        // Default keybind
        {
            let win = ModMask::M4.into();
            let win_shift = (ModMask::M4 | ModMask::SHIFT).into();
            config.add_keybind(win, KeyCode::Tab as u8, "focus-next");
            config.add_keybind(win_shift, KeyCode::Tab as u8, "focus-prev");
            config.add_keybind(win_shift, KeyCode::Q as u8, "exit");
            config.add_keybind(win, KeyCode::C as u8, "close");
            config.add_keybind(0, KeyCode::Super as u8, "super-press");
            config.add_keybind(win, KeyCode::Super as u8, "super-release");
            config.add_keybind(win, KeyCode::P as u8, "open-launcher");
        }

        config
    }
}

fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    let config: Rc<Config> = Config::default().into();

    // Connect with the X server (specified by $DISPLAY).
    let (conn, _) = x11rb::connect(display_name.into()).map_err(|_| Error::ConnectionFailed)?;
    let conn = Rc::new(conn);

    // Get a root window on the first screen.
    let screen = conn.setup().roots.get(0).expect("No screen");
    let root = screen.root;
    debug!("root = {}", root);

    let mut event_handler = EventHandler::new(conn.clone(), config.clone(), root)?;
    let mut wm = WinMan::new(conn.clone(), config, root)?;

    loop {
        let x11_event = conn.wait_for_event()?;
        let cmd = event_handler.handle(x11_event)?;
        wm.process(cmd)?;
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
