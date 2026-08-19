//! Playback state machine types.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackPhase {
    Idle,
    Opening { generation: u64, local: bool },
    Playing { generation: u64, local: bool },
    Stopping { generation: u64 },
    Failed { generation: u64 },
}

impl PlaybackPhase {
    pub fn generation(self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Opening { generation, .. }
            | Self::Playing { generation, .. }
            | Self::Stopping { generation }
            | Self::Failed { generation } => Some(generation),
        }
    }
}

pub enum PlaybackEvent {
    Status {
        text: String,
        generation: u64,
    },
    Title {
        title: String,
        generation: u64,
    },
    PlayOk {
        url: String,
        tap_url: Option<String>,
        generation: u64,
        local: bool,
    },
    StopOk {
        generation: u64,
    },
    Error {
        message: String,
        generation: u64,
    },
}
