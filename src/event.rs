use crate::error::Result;
use log::{trace, warn};

use x11rb::protocol::{randr, xproto::*, Event};

pub enum HandleResult {
    Consumed,
    Ignored,
}

pub trait EventHandler {
    fn handle_event(&mut self, event: Event) -> Result<HandleResult>;
}

macro_rules! event_handler_ignore {
    ($method_name:ident, $event_type:ty) => {
        fn $method_name(&mut self, e: $event_type) -> Result<HandleResult> {
            trace!("(default) {}: Ignore {:?}", stringify!($method_name), e);
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
    event_handler_ignore!(on_configure_notify, ConfigureNotifyEvent);
    event_handler_ignore!(on_randr_notify, randr::NotifyEvent);
}

impl<T: EventHandlerMethods> EventHandler for T {
    fn handle_event(&mut self, event: Event) -> Result<HandleResult> {
        trace!("event: {:?}", event);
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
            Event::ConfigureNotify(e) => self.on_configure_notify(e),
            Event::RandrNotify(e) => self.on_randr_notify(e),
            e => {
                warn!("unhandled event: {:?}", e);
                Ok(HandleResult::Ignored)
            }
        }
    }
}
