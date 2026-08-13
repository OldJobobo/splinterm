use super::super::{
    App, Arc, Connection, DataDeviceHandler, DataOfferHandler, DataSourceHandler, DndAction,
    DragOffer, PasteTarget, PrimarySelectionDeviceHandler, PrimarySelectionSourceHandler,
    QueueHandle, URI_LIST_MIME, WaylandSurface, WritePipe, ZwpPrimarySelectionDeviceV1,
    ZwpPrimarySelectionSourceV1, accepted_text_mime, accepted_uri_list_mime, copy_action_supported,
    spawn_clipboard_read, wl_data_device, wl_data_source, wl_surface, write_selection_payload,
};

fn reject_dropped_offer(offer: &DragOffer, reason: &'static str) {
    eprintln!("splinterm file drop rejected: {reason}");
    offer.accept_mime_type(offer.serial, None);
    offer.set_actions(DndAction::empty(), DndAction::empty());
    offer.destroy();
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
        x: f64,
        y: f64,
        surface: &wl_surface::WlSurface,
    ) {
        self.clipboard.drag_target = None;
        let Some(offer) = self
            .clipboard
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().drag_offer())
        else {
            return;
        };
        let mime = offer.with_mime_types(accepted_uri_list_mime);
        let target = (surface == self.surface.window.wl_surface())
            .then(|| self.file_drop_target_at((x, y)))
            .flatten();
        if mime.is_some() && target.is_some() {
            offer.accept_mime_type(offer.serial, mime);
            offer.set_actions(DndAction::Copy, DndAction::Copy);
            self.clipboard.drag_target = target;
        } else {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
    ) {
        self.clipboard.drag_target = None;
    }

    fn motion(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
        x: f64,
        y: f64,
    ) {
        let Some(offer) = self
            .clipboard
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().drag_offer())
        else {
            self.clipboard.drag_target = None;
            return;
        };
        let target = self.file_drop_target_at((x, y));
        if target.is_some() && offer.with_mime_types(accepted_uri_list_mime).is_some() {
            offer.accept_mime_type(offer.serial, Some(URI_LIST_MIME.into()));
            offer.set_actions(DndAction::Copy, DndAction::Copy);
            self.clipboard.drag_target = target;
        } else {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
            self.clipboard.drag_target = None;
        }
    }

    fn selection(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
    ) {
        self.clipboard.clipboard_offer = self
            .clipboard
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().selection_offer())
            .filter(|offer| offer.with_mime_types(accepted_text_mime).is_some());
    }

    fn drop_performed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
    ) {
        let offer = self
            .clipboard
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().drag_offer());
        let Some(offer) = offer else {
            eprintln!("splinterm file drop rejected: offer unavailable");
            self.clipboard.drag_target = None;
            return;
        };
        let Some(target) = self.clipboard.drag_target.take() else {
            reject_dropped_offer(&offer, "target unavailable");
            return;
        };
        if !copy_action_supported(offer.source_actions) {
            reject_dropped_offer(&offer, "source does not support copy");
            return;
        }
        let Some(mime) = offer.with_mime_types(accepted_uri_list_mime) else {
            reject_dropped_offer(&offer, "URI-list MIME unavailable");
            return;
        };
        // The compositor may deliver wl_data_device.drop before SCTK observes
        // the resulting wl_data_offer.action event. Reassert the already
        // accepted Copy-only outcome instead of treating the temporarily empty
        // cached selected_action as a rejection.
        offer.accept_mime_type(offer.serial, Some(mime.clone()));
        offer.set_actions(DndAction::Copy, DndAction::Copy);
        let Ok(pipe) = offer.receive(mime) else {
            reject_dropped_offer(&offer, "offer receive failed");
            return;
        };
        spawn_clipboard_read(
            pipe.into(),
            PasteTarget::FileDrop(target),
            target.input_generation,
            Some(offer),
            self.clipboard.clipboard_tx.clone(),
            self.platform.update_waker.clone(),
        );
    }
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        offer: &mut DragOffer,
        actions: DndAction,
    ) {
        if copy_action_supported(actions) {
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        } else if !actions.is_empty() {
            offer.set_actions(DndAction::empty(), DndAction::empty());
            self.clipboard.drag_target = None;
        }
    }

    fn selected_action(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _offer: &mut DragOffer,
        actions: DndAction,
    ) {
        if !actions.is_empty() && actions != DndAction::Copy {
            self.clipboard.drag_target = None;
        }
    }
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
        _mime: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &wl_data_source::WlDataSource,
        mime: String,
        write_pipe: WritePipe,
    ) {
        if accepted_text_mime(std::slice::from_ref(&mime)).is_none() {
            return;
        }
        if let Some((_, payload)) = self
            .clipboard
            .clipboard_sources
            .iter()
            .find(|(candidate, _)| candidate.inner() == source)
        {
            write_selection_payload(write_pipe, Arc::clone(payload));
        }
    }

    fn cancelled(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &wl_data_source::WlDataSource,
    ) {
        self.clipboard
            .clipboard_sources
            .retain(|(candidate, _)| candidate.inner() != source);
    }

    fn dnd_dropped(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
    ) {
    }

    fn dnd_finished(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
    ) {
    }

    fn action(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
        _action: DndAction,
    ) {
    }
}

impl PrimarySelectionDeviceHandler for App {
    fn selection(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        device: &ZwpPrimarySelectionDeviceV1,
    ) {
        self.clipboard.primary_offer = self
            .clipboard
            .primary_device
            .as_ref()
            .filter(|candidate| candidate.inner() == device)
            .and_then(|candidate| candidate.data().selection_offer())
            .filter(|offer| offer.with_mime_types(accepted_text_mime).is_some());
    }
}

impl PrimarySelectionSourceHandler for App {
    fn send_request(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
        mime: String,
        write_pipe: WritePipe,
    ) {
        if accepted_text_mime(std::slice::from_ref(&mime)).is_none() {
            return;
        }
        if let Some((_, payload)) = self
            .clipboard
            .primary_sources
            .iter()
            .find(|(candidate, _)| candidate.inner() == source)
        {
            write_selection_payload(write_pipe, Arc::clone(payload));
        }
    }

    fn cancelled(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
    ) {
        self.clipboard
            .primary_sources
            .retain(|(candidate, _)| candidate.inner() != source);
    }
}
