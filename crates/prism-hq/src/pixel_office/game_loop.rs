use std::time::Instant;

use gpui::{Context, WeakEntity};

use super::panel::PixelOfficePanel;

/// Drives the 60 Hz game loop for the Pixel Office panel.
pub struct GameLoop {
    pub last_tick: Instant,
}

impl GameLoop {
    pub fn new() -> Self {
        Self { last_tick: Instant::now() }
    }

    /// Spawn the tick loop as a detached task on the panel entity.
    pub fn start(entity: WeakEntity<PixelOfficePanel>, cx: &mut Context<PixelOfficePanel>) {
        cx.spawn(async move |_this, cx| {
            let mut last = Instant::now();
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let now = Instant::now();
                let dt = (now - last).as_secs_f32().min(0.1);
                last = now;

                let keep_going = entity
                    .update(cx, |panel, cx| {
                        panel.tick(dt, cx);
                    })
                    .is_ok();

                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }
}
