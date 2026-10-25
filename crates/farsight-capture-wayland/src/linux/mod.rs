//! The Linux implementation: portal session plus PipeWire stream.

mod eis;
mod portal;
mod stream;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::mpsc;

use crate::{CaptureError, InputHandle, Options, Started};

/// Events waiting for the caller. Small, because each frame is a full screen of pixels;
/// when it is full, new frames are dropped (their damage is carried forward).
const EVENT_QUEUE: usize = 4;

/// The size of the latest frame, written by the PipeWire thread and read by input injection.
#[derive(Debug, Default)]
pub(crate) struct FrameSize {
    width: AtomicU32,
    height: AtomicU32,
}

impl FrameSize {
    pub fn set(&self, width: u32, height: u32) {
        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);
    }

    pub fn get(&self) -> (u32, u32) {
        (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        )
    }
}

pub(crate) async fn start(options: Options) -> Result<Started, CaptureError> {
    let opened = portal::open(&options).await?;
    let (events_tx, events) = mpsc::channel(EVENT_QUEUE);
    let frame_size = Arc::new(FrameSize::default());
    let pipewire = match stream::spawn(
        opened.pipewire_fd,
        opened.node_id,
        events_tx.clone(),
        Arc::clone(&frame_size),
    )
    .await
    {
        Ok(pipewire) => pipewire,
        Err(error) => {
            opened.portal.close().await;
            return Err(error);
        }
    };
    let can_control = matches!(opened.portal, portal::Sharing::Full(_));
    let (input, input_rx) = InputHandle::channel();
    let supervisor = portal::Supervisor {
        opened_portal: opened.portal,
        node_id: opened.node_id,
        logical_size: opened.logical_size,
        frame_size,
        input: input_rx,
        events: events_tx,
        pipewire,
    };
    tokio::spawn(supervisor.run());
    Ok(Started {
        events,
        input,
        restore_token: opened.restore_token,
        can_control,
    })
}
