#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskKind {
    ProbeSetupDemo,
}

impl BackgroundTaskKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProbeSetupDemo => "probe setup demo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskRequest {
    DemoSuccess,
    DemoFailure,
}

impl BackgroundTaskRequest {
    #[must_use]
    pub const fn kind(self) -> BackgroundTaskKind {
        match self {
            Self::DemoSuccess | Self::DemoFailure => BackgroundTaskKind::ProbeSetupDemo,
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::DemoSuccess | Self::DemoFailure => "Probe setup demo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMessage {
    TaskStarted {
        kind: BackgroundTaskKind,
        title: String,
    },
    TaskProgress {
        kind: BackgroundTaskKind,
        step: usize,
        total_steps: usize,
        detail: String,
    },
    TaskSucceeded {
        kind: BackgroundTaskKind,
        title: String,
        lines: Vec<String>,
    },
    TaskFailed {
        kind: BackgroundTaskKind,
        title: String,
        detail: String,
    },
}
