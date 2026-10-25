//! GPU encoder handles aren't thread-safe, so each encoder lives on its own
//! thread; the agent's [`VideoEncoder`] sends it frames over a channel.

use std::sync::mpsc;

use farsight_codec::video::{VideoEncoder, VideoError};

/// One encoding session, created and used on its own thread.
pub(crate) trait Session {
    /// Encodes one frame of `0x00RRGGBB` pixels.
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError>;
}

/// Starts a thread named `name` running the session `start` makes, and
/// returns once it has started (or failed to).
pub(crate) fn spawn<S: Session>(
    name: &str,
    start: impl FnOnce() -> Result<S, VideoError> + Send + 'static,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    let (frames, frame_rx) = mpsc::sync_channel::<Vec<u32>>(1);
    let (result_tx, results) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut session = match start() {
                Ok(session) => session,
                Err(problem) => {
                    let _ = result_tx.send(Err(problem));
                    return;
                }
            };
            if result_tx.send(Ok(Vec::new())).is_err() {
                return;
            }
            for pixels in frame_rx {
                if result_tx.send(session.encode(&pixels)).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| VideoError(e.to_string()))?;
    // The thread reports whether the GPU accepted the session.
    results.recv().map_err(|e| VideoError(e.to_string()))??;
    Ok(Box::new(Worker { frames, results }))
}

/// The handle the agent holds.
struct Worker {
    frames: mpsc::SyncSender<Vec<u32>>,
    results: mpsc::Receiver<Result<Vec<u8>, VideoError>>,
}

impl VideoEncoder for Worker {
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        let stopped = || VideoError("the GPU encoder stopped".into());
        self.frames.send(pixels.to_vec()).map_err(|_| stopped())?;
        self.results.recv().map_err(|_| stopped())?
    }
}
