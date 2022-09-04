#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MCServerState {
    Stopped = 1,
    Starting = 2,
    Running = 3,
    Stopping = 4,
}

impl Default for MCServerState {
    fn default() -> Self {
        MCServerState::Stopped
    }
}
