//! The Wayland protocols the server offers, and the states Smithay keeps for them.

use smithay::{
    input::SeatState,
    reexports::wayland_server::DisplayHandle,
    wayland::{
        compositor::CompositorState, selection::data_device::DataDeviceState, shell::xdg::XdgShellState, shm::ShmState, text_input::TextInputManagerState,
    },
};

use crate::server::Server;

pub(crate) struct Protocols {
    pub compositor: CompositorState,
    pub shell: XdgShellState,
    pub activation: smithay::wayland::xdg_activation::XdgActivationState,
    pub shm: ShmState,
    pub dmabuf: smithay::wayland::dmabuf::DmabufState,
    /// Explicit sync (`linux-drm-syncobj-v1`), on a GPU that can wait on a timeline point by an eventfd.
    pub syncobj: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    pub seats: SeatState<Server>,
    pub data_device: DataDeviceState,
    pub primary_selection: smithay::wayland::selection::primary_selection::PrimarySelectionState,
    pub data_control: smithay::wayland::selection::ext_data_control::DataControlState,
}

impl Protocols {
    /// The protocols' globals, `clock` being the presentation clock's id. The outputs' globals and the
    /// seat's are the server's (see `Server::new`); dmabuf and explicit sync are offered by a host once it
    /// has its GPU.
    pub fn new(dh: &DisplayHandle, clock: u32) -> Protocols {
        TextInputManagerState::new::<Server>(dh);
        smithay::wayland::cursor_shape::CursorShapeManagerState::new::<Server>(dh);
        smithay::wayland::viewporter::ViewporterState::new::<Server>(dh);
        smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Server>(dh);
        let primary_selection = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<Server>(dh);
        let data_control = smithay::wayland::selection::ext_data_control::DataControlState::new::<Server, _>(dh, Some(&primary_selection), |_| true);
        smithay::wayland::presentation::PresentationState::new::<Server>(dh, clock);
        // FIFO, by which Mesa's Vulkan paces a game in FIFO mode instead of with a frame callback a
        // swapchain's teardown can destroy a second time from another thread as it is answered (which
        // under Wine takes a game's window when it switches mode). Its barriers are signalled with the
        // frame callbacks (see `Server::frame_callbacks`).
        smithay::wayland::fifo::FifoManagerState::new::<Server>(dh);
        // Commit timing: a commit to be shown no sooner than a time, held until the frame shown then is
        // composed (see `Server::release_timed_commits`). Mesa's Vulkan times FIFO frames by it.
        smithay::wayland::commit_timing::CommitTimingManagerState::new::<Server>(dh);
        crate::x11::shell::install(dh);
        // The screens' scale, 1, said as the fraction clients may size their buffers by (mpv's software
        // output sets a zero viewport size, a protocol error, when not told it).
        smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Server>(dh);
        // Dialogs marked as such (GTK 4, Qt 6, SDL 3), whatever their parent: their role (see `Server::facts`).
        smithay::wayland::shell::xdg::dialog::XdgDialogState::new::<Server>(dh);
        Protocols {
            compositor: CompositorState::new::<Server>(dh),
            shell: XdgShellState::new::<Server>(dh),
            activation: smithay::wayland::xdg_activation::XdgActivationState::new::<Server>(dh),
            shm: ShmState::new::<Server>(dh, vec![smithay::reexports::wayland_server::protocol::wl_shm::Format::Abgr8888]),
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            syncobj: None,
            seats: SeatState::new(),
            data_device: DataDeviceState::new::<Server>(dh),
            primary_selection,
            data_control,
        }
    }
}
