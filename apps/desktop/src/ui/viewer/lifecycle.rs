use super::*;

impl ViewerView {
    pub fn new(
        engine: Engine,
        mut frames_rx: Receiver<VideoFrame>,
        peer_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // The single-slot channel wakes GPUI directly. No polling thread or
        // unbounded intermediary is needed while the window is busy.
        cx.spawn_in(
            window,
            async move |this: gpui::WeakEntity<ViewerView>, cx| {
                while let Some(frame) = frames_rx.recv().await {
                    if this
                        .update_in(&mut *cx, |this, window, cx| {
                            this.handle_frame(frame, window);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                    // Always return time to the UI event loop. The latest-frame
                    // channel replaces stale pictures while key/mouse events run.
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                }
                let _ = this.update_in(&mut *cx, |this, window, cx| {
                    this.ended = true;
                    this.clip_clear_task = None;
                    this.pressed_keys.clear();
                    this.buttons = 0;
                    cx.notify();
                    window.refresh();
                });
            },
        )
        .detach();

        // Sampling is independent of frame arrivals: idle desktops show 0 fps,
        // and stalled updates still expose receive rate and queued input age.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if !this
                    .update(cx, |this, cx| {
                        if this.ended {
                            return false;
                        }
                        this.sample_diagnostics();
                        if this.info_visible {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        })
        .detach();

        // Focus-loss handling: release inputs still held remotely (macOS delivers
        // key-up only to the key window) and arm the local-clipboard clear timer
        // (settings.clipboard_clear_after_secs, 0 = disabled; cancelled on
        // reactivation or session end).
        let clear_secs = engine.settings().clipboard_clear_after_secs;
        let sub_activation = cx.observe_window_activation(window, move |this, window, cx| {
            if window.is_window_active() {
                // Focus regained: dropping the task cancels the pending clear.
                this.clip_clear_task = None;
                return;
            }
            this.release_held_inputs();
            if clear_secs == 0 || this.ended || this.clip_clear_task.is_some() {
                return;
            }
            let engine = this.engine.clone();
            this.clip_clear_task = Some(cx.spawn(async move |_this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(clear_secs))
                    .await;
                // Clears the LOCAL clipboard; the remote side is not touched.
                engine.clear_local_clipboard();
            }));
        });

        let recovery = engine.client_recovery();
        let mut recovery_rx = recovery.subscribe();
        cx.spawn(async move |this, cx| {
            while recovery_rx.changed().await.is_ok() {
                let state = recovery_rx.borrow_and_update().clone();
                if this
                    .update(cx, |this, cx| {
                        if state != removent_client::recovery::State::Connected {
                            this.pressed_keys.clear();
                            this.buttons = 0;
                            this.sent_modifiers = KeyModifiers::empty();
                            this.clip_clear_task = None;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if matches!(
                    state,
                    removent_client::recovery::State::Ended
                        | removent_client::recovery::State::Failed(_)
                ) {
                    break;
                }
            }
        })
        .detach();

        Self {
            recovery,
            input_epoch: 0,
            canvas_bounds: Default::default(),
            session_generation: engine.client_generation(),
            engine,
            current: None,
            width: 0,
            height: 0,
            fps_counter: 0,
            fps_shown: 0.,
            last_fps_tick: Instant::now(),
            info_visible: false,
            info_scroll: gpui::ScrollHandle::new(),
            started: Instant::now(),
            last_frame: None,
            diagnostics: ClientDiagnostics::default(),
            received_bytes: 0,
            received_frames: 0,
            receive_rate: 0.,
            receive_fps: 0.,
            peer_name,
            toolbar_until: None,
            toolbar_seq: 0,
            ended: false,
            focus,
            buttons: 0,
            sent_modifiers: KeyModifiers::empty(),
            pressed_keys: HashSet::new(),
            last_mouse: None,
            toolbar_hide_seq: 0,
            clip_clear_task: None,
            _subscriptions: vec![sub_activation],
        }
    }

    /// GPUI RenderImage consumes BGRA bytes, despite using an RgbaImage container.
    pub(super) fn handle_frame(&mut self, frame: VideoFrame, window: &mut Window) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        if let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.data) {
            let epoch = self.recovery.connection_epoch();
            if self.input_epoch != epoch {
                // A fast reconnect can coalesce watch notifications. Never
                // carry held inputs into a replacement transport in that case.
                self.input_epoch = epoch;
                self.pressed_keys.clear();
                self.buttons = 0;
                self.sent_modifiers = KeyModifiers::empty();
                self.last_mouse = None;
            }
            let next = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
            if let Some(previous) = self.current.replace(next) {
                // RenderImage IDs are unique. GPUI's sprite atlas does not
                // evict them when the Arc drops, so release each retired frame.
                let _ = window.drop_image(previous);
            }
            if self.width > 0
                && self.height > 0
                && let Some((x, y)) = &mut self.last_mouse
            {
                *x *= frame.width as f32 / self.width as f32;
                *y *= frame.height as f32 / self.height as f32;
            }
            self.engine
                .set_frame_geometry(self.session_generation, frame.width, frame.height);
            self.width = frame.width;
            self.height = frame.height;
            self.fps_counter += 1;
            self.last_frame = Some(Instant::now());
        }
    }

    pub(super) fn sample_diagnostics(&mut self) {
        let seconds = self.last_fps_tick.elapsed().as_secs_f64().max(0.001);
        self.fps_shown = self.fps_counter as f64 / seconds;
        self.fps_counter = 0;
        self.last_fps_tick = Instant::now();
        self.diagnostics = self.engine.client_diagnostics(self.session_generation);
        if let Some(stats) = self.diagnostics.vnc {
            self.receive_rate =
                stats.received_bytes.saturating_sub(self.received_bytes) as f64 / seconds;
            self.receive_fps =
                stats.received_frames.saturating_sub(self.received_frames) as f64 / seconds;
            self.received_bytes = stats.received_bytes;
            self.received_frames = stats.received_frames;
        }
    }

    pub(super) fn toggle_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info_visible = !self.info_visible;
        self.diagnostics = self.engine.client_diagnostics(self.session_generation);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn disconnect(&mut self, window: &mut Window) {
        self.clip_clear_task = None;
        self.pressed_keys.clear();
        self.buttons = 0;
        self.engine.disconnect_client_if(self.session_generation);
        window.remove_window();
    }

    /// Input is forwarded only while a live session is streaming frames.
    pub(super) fn input_active(&self) -> bool {
        !self.ended
            && self.recovery.state() == removent_client::recovery::State::Connected
            && self.engine.client_input_ready(self.session_generation)
            && self.width > 0
            && self.height > 0
    }
}
