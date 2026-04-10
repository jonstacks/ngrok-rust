/// Default per-stream receive window / buffer max size (256 KB).
pub const DEFAULT_MAX_WINDOW_SIZE: u32 = 0x40000;

/// Default accept backlog for incoming streams.
pub const DEFAULT_ACCEPT_BACKLOG: usize = 128;

/// Default write frame queue depth.
pub const DEFAULT_WRITE_QUEUE_DEPTH: usize = 64;

/// Configuration for a muxado session.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum per-stream receive window and buffer size in bytes.
    pub max_window_size: u32,
    /// Maximum number of streams waiting in the accept queue.
    pub accept_backlog: usize,
    /// Depth of the outbound frame write queue.
    pub write_queue_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_window_size: DEFAULT_MAX_WINDOW_SIZE,
            accept_backlog: DEFAULT_ACCEPT_BACKLOG,
            write_queue_depth: DEFAULT_WRITE_QUEUE_DEPTH,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = Config::default();
        assert_eq!(cfg.max_window_size, 0x40000);
        assert_eq!(cfg.accept_backlog, 128);
        assert_eq!(cfg.write_queue_depth, 64);
    }
}
