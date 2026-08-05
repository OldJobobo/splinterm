use super::super::{
    App, Arc, Connection, DataDeviceHandler, DataOfferHandler, DataSourceHandler, DndAction,
    DragOffer, PrimarySelectionDeviceHandler, PrimarySelectionSourceHandler, QueueHandle,
    WritePipe, ZwpPrimarySelectionDeviceV1, ZwpPrimarySelectionSourceV1, accepted_text_mime,
    wl_data_device, wl_data_source, wl_surface, write_selection_payload,
};

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
        _x: f64,
        _y: f64,
        _surface: &wl_surface::WlSurface,
    ) {
        if let Some(offer) = self
            .clipboard
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().drag_offer())
        {
            offer.accept_mime_type(0, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
    ) {
    }

    fn motion(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
        _x: f64,
        _y: f64,
    ) {
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
        _data_device: &wl_data_device::WlDataDevice,
    ) {
    }
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        offer: &mut DragOffer,
        _actions: DndAction,
    ) {
        offer.set_actions(DndAction::empty(), DndAction::empty());
    }

    fn selected_action(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _offer: &mut DragOffer,
        _actions: DndAction,
    ) {
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
