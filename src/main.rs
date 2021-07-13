mod error;

use std::collections::HashMap;
use std::rc::Rc;

use error::{Error, Result};
use log::{debug, error, info, trace, warn};

use x11rb::connection::Connection;
use x11rb::protocol::{
    randr::{self, ConnectionExt as _},
    xproto::*,
    Event,
};
use Window as Wid;

#[derive(Debug)]
pub enum HandleResult {
    Consumed,
    Ignored,
}

pub trait EventHandler {
    fn handle_event(&mut self, event: Event) -> Result<HandleResult>;
}

macro_rules! event_handler_ignore {
    ($method_name:ident, $event_type:ty) => {
        fn $method_name(&mut self, _: $event_type) -> Result<HandleResult> {
            Ok(HandleResult::Ignored)
        }
    };
}

pub trait EventHandlerMethods {
    event_handler_ignore!(on_key_press, KeyPressEvent);
    event_handler_ignore!(on_key_release, KeyReleaseEvent);
    event_handler_ignore!(on_button_press, ButtonPressEvent);
    event_handler_ignore!(on_button_release, ButtonReleaseEvent);
    event_handler_ignore!(on_map_request, MapRequestEvent);
    event_handler_ignore!(on_map_notify, MapNotifyEvent);
    event_handler_ignore!(on_unmap_notify, UnmapNotifyEvent);
    event_handler_ignore!(on_create_notify, CreateNotifyEvent);
    event_handler_ignore!(on_destroy_notify, DestroyNotifyEvent);
}

impl<T: EventHandlerMethods> EventHandler for T {
    fn handle_event(&mut self, event: Event) -> Result<HandleResult> {
        match event {
            Event::KeyPress(e) => self.on_key_press(e),
            Event::KeyRelease(e) => self.on_key_release(e),
            Event::ButtonPress(e) => self.on_button_press(e),
            Event::ButtonRelease(e) => self.on_button_release(e),
            Event::MapRequest(e) => self.on_map_request(e),
            Event::MapNotify(e) => self.on_map_notify(e),
            Event::UnmapNotify(e) => self.on_unmap_notify(e),
            Event::CreateNotify(e) => self.on_create_notify(e),
            Event::DestroyNotify(e) => self.on_destroy_notify(e),
            e => {
                warn!("unhandled event: {:?}", e);
                Ok(HandleResult::Ignored)
            }
        }
    }
}

#[derive(Default)]
pub struct EventRouter {
    list: Vec<Box<dyn EventHandler>>,
}
impl EventRouter {
    pub fn add_handler(&mut self, h: Box<dyn EventHandler>) {
        self.list.push(h);
    }
}
impl EventHandler for EventRouter {
    fn handle_event(&mut self, event: Event) -> Result<HandleResult> {
        trace!("event: {:?}", event);
        for h in self.list.iter_mut() {
            match h.handle_event(event.clone()) {
                Ok(HandleResult::Ignored) => {
                    continue;
                }
                Ok(HandleResult::Consumed) => {
                    return Ok(HandleResult::Consumed);
                }
                err => return err,
            }
        }
        Ok(HandleResult::Ignored)
    }
}

impl std::fmt::Debug for EventRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EventRouter {{...}}")
    }
}

use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct WindowState {
    mapped: bool,
}

#[derive(Debug)]
struct WinMan<C: Connection> {
    conn: Rc<C>,
    config: Rc<Config>,
    root: Wid,
    event_router: Arc<Mutex<EventRouter>>,
    windows: HashMap<Wid, WindowState>,
    monitor_size: (u16, u16),
    border_visible: bool,
}

impl<C: Connection> WinMan<C> {
    pub fn new(
        conn: Rc<C>,
        config: Rc<Config>,
        root: Wid,
        event_router: Arc<Mutex<EventRouter>>,
    ) -> Result<Self> {
        let mut wm = Self {
            conn,
            config,
            root,
            event_router,
            windows: HashMap::new(),
            monitor_size: (0, 0),
            border_visible: false,
        };
        wm.init()?;

        Ok(wm)
    }

    fn init(&mut self) -> Result<()> {
        // Become a window manager of the root window.
        let mask = EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT;
        let aux = ChangeWindowAttributesAux::new().event_mask(mask);
        self.conn
            .change_window_attributes(self.root, &aux)?
            .check()
            .map_err(|_| Error::WmAlreadyExists)?;

        // Grab keys
        for (_, modif, keycode) in self.config.bounded_keys() {
            self.conn
                .grab_key(
                    true,
                    self.root,
                    modif,
                    keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()
                .map_err(|_| Error::KeyAlreadyGrabbed)?;
        }

        // Grab mouse buttons
        let event_mask: u32 = (EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE).into();
        self.conn
            .grab_button(
                false,
                self.root,
                event_mask as u16,
                GrabMode::SYNC,
                GrabMode::ASYNC,
                self.root,
                x11rb::NONE,
                ButtonIndex::M1,
                ModMask::ANY,
            )?
            .check()
            .map_err(|_| Error::ButtonAlreadyGrabbed)?;

        // Receive RROutputChangeNotifyEvent
        self.conn
            .randr_select_input(self.root, randr::NotifyMask::OUTPUT_CHANGE)?;

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

    fn process_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Exit => return Err(Error::Quit),
            Command::ShowBorder => {
                self.border_visible = true;
                self.refresh_layout_horizontal_split()?;
            }
            Command::HideBorder => {
                self.border_visible = false;
                self.refresh_layout_horizontal_split()?;
            }
            Command::Close => {
                let focused = self.conn.get_input_focus()?.reply()?.focus;
                self.focus_next()?;
                self.conn.destroy_window(focused)?;
                self.conn.flush()?;
            }
            Command::FocusNext => {
                self.focus_next()?;
                self.refresh_layout_horizontal_split()?;
            }
            Command::FocusPrev => {
                warn!("not yet implemented");
            }
            Command::OpenLauncher => {
                let _ = std::process::Command::new(self.config.launcher.as_str()).spawn();
            }
        }
        Ok(())
    }
}

impl<C: Connection> EventHandlerMethods for WinMan<C> {
    fn on_key_press(&mut self, e: KeyPressEvent) -> Result<HandleResult> {
        if let Some(cmd) = self
            .config
            .get_keybind(KeybindAction::Press, e.state, e.detail)
        {
            debug!("on_key_press: cmd = {:?}", cmd);
            self.process_command(cmd)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_key_release(&mut self, e: KeyReleaseEvent) -> Result<HandleResult> {
        if let Some(cmd) = self
            .config
            .get_keybind(KeybindAction::Release, e.state, e.detail)
        {
            debug!("on_key_release: cmd = {:?}", cmd);
            self.process_command(cmd)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_button_press(&mut self, e: ButtonPressEvent) -> Result<HandleResult> {
        self.conn
            .allow_events(Allow::REPLAY_POINTER, x11rb::CURRENT_TIME)?
            .check()?;
        if e.child != x11rb::NONE {
            debug!("set_input_focus");
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, e.child, x11rb::CURRENT_TIME)?;
            self.conn.flush()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_map_request(&mut self, req: MapRequestEvent) -> Result<HandleResult> {
        self.conn.map_window(req.window)?;
        self.conn.flush()?;
        Ok(HandleResult::Consumed)
    }

    fn on_map_notify(&mut self, notif: MapNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            self.map_window(notif.window)?;
            self.conn.set_input_focus(
                InputFocus::POINTER_ROOT,
                notif.window,
                x11rb::CURRENT_TIME,
            )?;
            self.conn.flush()?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }

    fn on_unmap_notify(&mut self, notif: UnmapNotifyEvent) -> Result<HandleResult> {
        if self.windows.contains_key(&notif.window) {
            self.unmap_window(notif.window)?;
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }
    fn on_create_notify(&mut self, notif: CreateNotifyEvent) -> Result<HandleResult> {
        if !notif.override_redirect {
            let state = WindowState { mapped: false };
            self.windows.insert(notif.window, state);
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
        }
    }
    fn on_destroy_notify(&mut self, notif: DestroyNotifyEvent) -> Result<HandleResult> {
        if self.windows.contains_key(&notif.window) {
            self.windows.remove(&notif.window);
            Ok(HandleResult::Consumed)
        } else {
            Ok(HandleResult::Ignored)
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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Command {
    Exit,
    ShowBorder,
    HideBorder,
    Close,
    FocusNext,
    FocusPrev,
    OpenLauncher,
}

use std::cell::RefCell;

#[derive(Debug)]
pub struct Config {
    keybind: RefCell<HashMap<(KeybindAction, u16, u8), Command>>,
    launcher: String,
}

impl Config {
    pub fn add_keybind(&self, on: KeybindAction, modifier: u16, keycode: u8, cmd: Command) {
        let mut keybind = self.keybind.borrow_mut();
        keybind.insert((on, modifier, keycode), cmd);
    }

    pub fn get_keybind(&self, on: KeybindAction, modifier: u16, keycode: u8) -> Option<Command> {
        let keybind = self.keybind.borrow();
        keybind.get(&(on, modifier, keycode)).cloned()
    }

    pub fn bounded_keys(&self) -> Vec<(KeybindAction, u16, u8)> {
        let keybind = self.keybind.borrow();
        let mut keys = Vec::new();
        for &(on, m, c) in keybind.keys() {
            keys.push((on, m, c));
        }
        keys
    }
}

impl Default for Config {
    fn default() -> Self {
        let config = Config {
            keybind: Default::default(),
            launcher: "/usr/bin/dmenu_run".to_owned(),
        };

        // Default keybind
        {
            let win = ModMask::M4.into();
            let win_shift = (ModMask::M4 | ModMask::SHIFT).into();
            config.add_keybind(
                KeybindAction::Press,
                win,
                KeyCode::Tab as u8,
                Command::FocusNext,
            );
            config.add_keybind(
                KeybindAction::Press,
                win_shift,
                KeyCode::Tab as u8,
                Command::FocusPrev,
            );
            config.add_keybind(
                KeybindAction::Press,
                win_shift,
                KeyCode::Q as u8,
                Command::Exit,
            );
            config.add_keybind(KeybindAction::Press, win, KeyCode::C as u8, Command::Close);
            config.add_keybind(
                KeybindAction::Press,
                0,
                KeyCode::Super as u8,
                Command::ShowBorder,
            );
            config.add_keybind(
                KeybindAction::Release,
                win,
                KeyCode::Super as u8,
                Command::HideBorder,
            );
            config.add_keybind(
                KeybindAction::Press,
                win,
                KeyCode::P as u8,
                Command::OpenLauncher,
            );
        }

        config
    }
}

pub fn start<S>(display_name: S) -> Result<()>
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

    let router = Arc::new(Mutex::new(EventRouter::default()));
    {
        let wm = WinMan::new(conn.clone(), config, root, router.clone())?;
        let mut router = router.lock().unwrap();
        router.add_handler(Box::new(wm));
    }

    loop {
        let x11_event = conn.wait_for_event()?;
        let mut router = router.lock().unwrap();
        router.handle_event(x11_event.clone())?;
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
