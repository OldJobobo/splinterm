use super::super::{
    App, Connection, OutputHandler, OutputState, QueueHandle, Shm, ShmHandler, note_output_leave,
    wl_output,
};

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.platform.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn update_output(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.platform.entered_outputs.last() == Some(&output) {
            if let Err(error) = self.refresh_output_dpi(&output, queue_handle) {
                self.scheduling.fail(error);
            }
        }
    }
    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let was_most_recent = note_output_leave(&mut self.platform.entered_outputs, &output);
        self.platform.output_count = self.platform.entered_outputs.len();
        if was_most_recent {
            if let Some(current) = self.platform.entered_outputs.last().cloned() {
                if let Err(error) = self.refresh_output_dpi(&current, queue_handle) {
                    self.scheduling.fail(error);
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.platform.shm
    }
}
