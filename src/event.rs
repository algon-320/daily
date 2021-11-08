use crate::error::Result;
use log::{trace, warn};

use x11rb::protocol::{randr, xproto::*, Event};

pub trait EventHandler {
    fn handle_event(&mut self, event: Event) -> Result<()>;
}

macro_rules! event_handler_ignore {
    ($method_name:ident, $event_type:ty) => {
        fn $method_name(&mut self, e: $event_type) -> Result<()> {
            trace!("(default) {}: Ignore {:?}", stringify!($method_name), e);
            Ok(())
        }
    };
}

pub trait EventHandlerMethods {
    event_handler_ignore!(on_key_press, KeyPressEvent);
    event_handler_ignore!(on_key_release, KeyReleaseEvent);
    event_handler_ignore!(on_button_press, ButtonPressEvent);
    event_handler_ignore!(on_button_release, ButtonReleaseEvent);
    event_handler_ignore!(on_motion_notify, MotionNotifyEvent);
    event_handler_ignore!(on_map_request, MapRequestEvent);
    event_handler_ignore!(on_map_notify, MapNotifyEvent);
    event_handler_ignore!(on_unmap_notify, UnmapNotifyEvent);
    event_handler_ignore!(on_create_notify, CreateNotifyEvent);
    event_handler_ignore!(on_destroy_notify, DestroyNotifyEvent);
    event_handler_ignore!(on_configure_request, ConfigureRequestEvent);
    event_handler_ignore!(on_configure_notify, ConfigureNotifyEvent);
    event_handler_ignore!(on_expose, ExposeEvent);
    event_handler_ignore!(on_focus_in, FocusInEvent);
    event_handler_ignore!(on_focus_out, FocusInEvent);
    event_handler_ignore!(on_client_message, ClientMessageEvent);
    event_handler_ignore!(on_randr_notify, randr::NotifyEvent);
}

impl<T: EventHandlerMethods> EventHandler for T {
    fn handle_event(&mut self, event: Event) -> Result<()> {
        trace!("event: {:?}", event);
        match event {
            Event::KeyPress(e) => self.on_key_press(e),
            Event::KeyRelease(e) => self.on_key_release(e),
            Event::ButtonPress(e) => self.on_button_press(e),
            Event::ButtonRelease(e) => self.on_button_release(e),
            Event::MotionNotify(e) => self.on_motion_notify(e),
            Event::MapRequest(e) => self.on_map_request(e),
            Event::MapNotify(e) => self.on_map_notify(e),
            Event::UnmapNotify(e) => self.on_unmap_notify(e),
            Event::CreateNotify(e) => self.on_create_notify(e),
            Event::DestroyNotify(e) => self.on_destroy_notify(e),
            Event::ConfigureRequest(e) => self.on_configure_request(e),
            Event::ConfigureNotify(e) => self.on_configure_notify(e),
            Event::Expose(e) => self.on_expose(e),
            Event::FocusIn(e) => self.on_focus_in(e),
            Event::FocusOut(e) => self.on_focus_out(e),
            Event::ClientMessage(e) => self.on_client_message(e),
            Event::RandrNotify(e) => self.on_randr_notify(e),
            e => {
                warn!("unhandled event: {:?}", e);
                Ok(())
            }
        }
    }
}
