#![allow(dead_code)]

use log::{debug, error, info};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window as Wid, *};
use x11rb::rust_connection::RustConnection;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::event::{EventHandler as _, EventHandlerMethods};
use crate::spawn_named_thread;

#[derive(Debug)]
pub enum Request {
    GetWindowId,
    Configure { geometry: Rectangle },
    UpdateContent { content: Content },
    Show,
    Hide,
}

#[derive(Debug)]
pub enum Response {
    WindowId(u32),
    Success,
    Error { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct Content {
    pub max_screen: usize,
    pub current_screen: usize,
}

#[derive(Debug)]
pub struct BarHandle {
    tx: Sender<Request>,
    rx: Receiver<Response>,
}

impl BarHandle {
    pub fn new(ctx: &Context, _id: usize) -> Self {
        let (req_tx, req_rx) = channel::<Request>();
        let (resp_tx, resp_rx) = channel::<Response>();

        thread_main(ctx, req_rx, resp_tx);

        Self {
            tx: req_tx,
            rx: resp_rx,
        }
    }

    fn send(&mut self, req: Request) -> Result<()> {
        self.tx.send(req).map_err(|_| Error::BrokenChannel)
    }
    fn recv(&mut self) -> Result<Response> {
        self.rx.recv().map_err(|_| Error::BrokenChannel)
    }

    pub fn configure(&mut self, x: i16, y: i16, width: u16, height: u16) -> Result<()> {
        let req = Request::Configure {
            geometry: Rectangle {
                x,
                y,
                width,
                height,
            },
        };
        self.send(req)?;
        match self.recv()? {
            Response::Success => Ok(()),
            resp => panic!("Invalid Mesage: {:?}", resp),
        }
    }

    pub fn update_content(&mut self, content: Content) -> Result<()> {
        self.send(Request::UpdateContent { content })?;
        match self.recv()? {
            Response::Success => Ok(()),
            resp => panic!("Invalid Mesage: {:?}", resp),
        }
    }

    pub fn get_window_id(&mut self) -> Result<Wid> {
        self.send(Request::GetWindowId)?;
        match self.recv()? {
            Response::WindowId(wid) => Ok(wid),
            resp => panic!("Invalid Mesage: {:?}", resp),
        }
    }

    pub fn show(&mut self) -> Result<()> {
        self.unit_request(Request::Show)
    }

    pub fn hide(&mut self) -> Result<()> {
        self.unit_request(Request::Hide)
    }

    fn unit_request(&mut self, req: Request) -> Result<()> {
        self.send(req)?;
        match self.recv()? {
            Response::Success => Ok(()),
            resp => panic!("Invalid Mesage: {:?}", resp),
        }
    }
}

enum BarEvent {
    Request(Request),
    Alarm,
    Disconnected,
    X11(x11rb::protocol::Event),
}

/// Consume X11 events and redirect it to tx.
fn thread_x11_event_consumer(tx: Sender<BarEvent>, conn: Arc<RustConnection>) {
    spawn_named_thread("bar-x11".to_owned(), move || loop {
        let event = conn.wait_for_event().expect("cannot get event");
        tx.send(BarEvent::X11(event)).expect("rx has been closed");
    });
}

/// Read a request from the receiver and redirect it to tx.
fn thread_daily_request_redirector(tx: Sender<BarEvent>, rx: Receiver<Request>) {
    spawn_named_thread("bar-request".to_owned(), move || loop {
        let event = match rx.recv() {
            Ok(req) => BarEvent::Request(req),
            Err(err) => {
                error!("cannot receive a request: {:?}", err);
                BarEvent::Disconnected
            }
        };
        tx.send(event).expect("rx has been closed");
    });
}

/// Deliver a `BarEvent::Alarm` every 10 seconds.
fn thread_timer(tx: Sender<BarEvent>) {
    spawn_named_thread("bar-timer".to_owned(), move || loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        tx.send(BarEvent::Alarm).expect("rx has been closed");
    });
}

/// Process `BarEvent`s
fn thread_main(ctx: &Context, request_rx: Receiver<Request>, response_tx: Sender<Response>) {
    let display = ctx.display.clone();
    spawn_named_thread("bar-main".to_owned(), move || -> Result<()> {
        let display = display.as_deref();
        // Use a dedicated connection for this bar.
        let (conn, _) =
            RustConnection::connect(display).expect("cannot establish a connection to X server");
        let conn = Arc::new(conn);

        let (tx, rx) = channel::<BarEvent>();

        // Spawn threads
        thread_daily_request_redirector(tx.clone(), request_rx);
        thread_timer(tx.clone());
        thread_x11_event_consumer(tx, conn.clone());

        let mut bar = Bar::new(conn)?;

        while let Ok(event) = rx.recv() {
            match event {
                BarEvent::Request(cmd) => match cmd {
                    Request::Configure { geometry } => {
                        bar.configure(geometry)?;
                        response_tx.send(Response::Success).unwrap();
                    }
                    Request::UpdateContent { content } => {
                        bar.update_content(content)?;
                        response_tx.send(Response::Success).unwrap();
                    }
                    Request::GetWindowId => {
                        response_tx.send(Response::WindowId(bar.wid)).unwrap();
                    }
                    Request::Show => {
                        bar.show()?;
                        response_tx.send(Response::Success).unwrap();
                    }
                    Request::Hide => {
                        bar.hide()?;
                        response_tx.send(Response::Success).unwrap();
                    }
                },

                BarEvent::Alarm => {
                    bar.draw()?;
                }

                BarEvent::Disconnected => {
                    break;
                }

                BarEvent::X11(event) => {
                    bar.handle_event(event)?;
                }
            }
        }
        info!("bar stop");
        Ok(())
    });
}

struct Bar {
    conn: Arc<RustConnection>,
    wid: Wid,
    gc: Gcontext,
    mon: Rectangle,
    content: Content,
}

impl Bar {
    fn new(conn: Arc<RustConnection>) -> Result<Self> {
        let root = conn.setup().roots[0].root;

        let wid = conn.generate_id()?;
        let depth = x11rb::COPY_DEPTH_FROM_PARENT;
        let class = WindowClass::INPUT_OUTPUT;
        let visual = x11rb::COPY_FROM_PARENT;
        let aux = CreateWindowAux::new()
            .background_pixel(0x4e4b61)
            .event_mask(EventMask::EXPOSURE)
            .override_redirect(1);
        conn.create_window(depth, wid, root, -1, -1, 1, 1, 0, class, visual, &aux)?;
        debug!("window={} created", wid);

        let gc = conn.generate_id()?;
        let aux = CreateGCAux::new().background(0x4e4b61).foreground(0xd2ca9c);
        conn.create_gc(gc, wid, &aux)?;

        conn.flush()?;

        Ok(Self {
            conn,
            wid,
            gc,
            mon: Rectangle {
                x: -1,
                y: -1,
                width: 1,
                height: 1,
            },
            content: Content::default(),
        })
    }

    fn configure(&mut self, mon: Rectangle) -> Result<()> {
        debug!("configure {:?}", mon);
        self.mon = mon;
        let aux = ConfigureWindowAux::new()
            .x(mon.x as i32)
            .y(mon.y as i32)
            .width(mon.width as u32)
            .height(16) // FIXME
            .stack_mode(StackMode::BELOW); // Bottom of the stack
        self.conn.configure_window(self.wid, &aux)?;
        self.conn.flush()?;
        self.draw()?;
        Ok(())
    }

    fn update_content(&mut self, content: Content) -> Result<()> {
        self.content = content;
        self.draw()?;
        Ok(())
    }

    fn show(&mut self) -> Result<()> {
        self.conn.map_window(self.wid)?;
        self.conn.flush()?;
        self.draw()?;
        Ok(())
    }

    fn hide(&mut self) -> Result<()> {
        self.conn.unmap_window(self.wid)?;
        self.conn.flush()?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        debug!("draw: mon={:?}, content={:?}", self.mon, self.content);
        let w = self.mon.width as i16;

        let bar = self.wid;
        let gc = self.gc;

        let color_bg = 0x4e4b61;

        // Lines
        let aux = ChangeGCAux::new().foreground(0x69656d);
        self.conn.change_gc(gc, &aux)?;

        let p1 = Point { x: 0, y: 14 };
        let p2 = Point { x: 0, y: 0 };
        let p3 = Point { x: w - 2, y: 0 };
        self.conn
            .poly_line(CoordMode::ORIGIN, bar, gc, &[p1, p2, p3])?;

        let aux = ChangeGCAux::new().foreground(0x1a1949);
        self.conn.change_gc(gc, &aux)?;

        let p1 = Point { x: 1, y: 15 };
        let p2 = Point { x: w - 1, y: 15 };
        let p3 = Point { x: w - 1, y: 1 };
        self.conn
            .poly_line(CoordMode::ORIGIN, bar, gc, &[p1, p2, p3])?;

        // Digits
        let offset_x = 2;
        let offset_y = 5;
        let cont = &self.content;
        for i in 0..cont.max_screen {
            let color1;
            let color2;
            if i == cont.current_screen {
                color1 = 0x00f080;
                color2 = 0x007840;
            } else {
                color1 = 0xd2ca9c;
                color2 = 0x9d9784;
            }

            let x = offset_x + (i * 12) as i16;
            let y = offset_y;
            let digit = b'1' + (i as u8);
            draw_digit(&*self.conn, bar, gc, x, y, digit, color1, color2)?;
        }

        // clock
        use chrono::prelude::*;
        let mut x = w - 136;
        let y = 5;

        let aux = ChangeGCAux::new().foreground(color_bg).background(color_bg);
        self.conn.change_gc(gc, &aux)?;

        let rect = Rectangle {
            x,
            y,
            width: (6 + 2) * 16,
            height: 6,
        };
        self.conn.poly_fill_rectangle(bar, gc, &[rect])?;

        let (color1, color2) = (0xd2ca9c, 0x9d9784);
        let now = chrono::Local::now();
        let date = now.date();
        let time = now.time();

        let date_time = format!(
            "{:04}/{:02}/{:02} {:02}:{:02}",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute()
        );
        for &b in date_time.as_bytes() {
            draw_digit(&*self.conn, bar, gc, x, y, b, color1, color2)?;
            x += 8;
        }

        self.conn.flush()?;
        Ok(())
    }
}

impl EventHandlerMethods for Bar {
    fn on_expose(&mut self, _e: ExposeEvent) -> Result<()> {
        self.draw()?;
        Ok(())
    }
}

fn draw_digit<C: Connection>(
    conn: &C,
    wid: Drawable,
    gc: Gcontext,
    x: i16,
    y: i16,
    ascii_digit: u8,
    color1: u32,
    color2: u32,
) -> Result<()> {
    const DIGITS: [[u32; 6 * 6]; 10 + 3] = include!("digits.txt");

    let digit = if (b'0'..=b'9').contains(&ascii_digit) {
        ascii_digit - b'0'
    } else if ascii_digit == b':' {
        10
    } else if ascii_digit == b'/' {
        11
    } else if ascii_digit == b' ' {
        12
    } else {
        panic!(
            "unsupported char: {}",
            char::from_u32(ascii_digit as u32).unwrap()
        );
    };

    let mut ps1 = Vec::new();
    let mut ps2 = Vec::new();
    for (p, &e) in DIGITS[digit as usize].iter().enumerate() {
        let (yi, xi) = (p / 6, p % 6);
        let point = Point {
            x: x + xi as i16,
            y: y + yi as i16,
        };
        if e == 1 {
            ps1.push(point);
        } else if e == 2 {
            ps2.push(point);
        }
    }

    if !ps1.is_empty() {
        let aux = ChangeGCAux::new().foreground(color1);
        conn.change_gc(gc, &aux)?;
        conn.poly_point(CoordMode::ORIGIN, wid, gc, &ps1)?;
    }

    if !ps2.is_empty() {
        let aux = ChangeGCAux::new().foreground(color2);
        conn.change_gc(gc, &aux)?;
        conn.poly_point(CoordMode::ORIGIN, wid, gc, &ps2)?;
    }

    Ok(())
}
