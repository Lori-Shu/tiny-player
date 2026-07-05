//! The presentation module manages data synchronization and presentation
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use eframe::wgpu::Texture;
use ffmpeg_the_third::{
    Rational,
    frame::{Audio, Video},
};
use flume::Receiver;
use tokio::{
    runtime::Handle,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::{future::FutureExt, sync::CancellationToken};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult,
    audio_playback::AudioPlayer,
    decode_engine::{MainStream, TinyDecoder},
    post_process::Transcoder,
    whispercpp_transcriber::{Transcriber, UsedModel},
};
pub const PLAY_SAMPLE_RATE: u32 = 48000;
pub struct PresentDataManager {
    audio_thread_handle: Option<JoinHandle<()>>,
    video_thread_handle: Option<JoinHandle<()>>,
    cancellation_token: Arc<CancellationToken>,
    audio_play_context: AudioPlayContext,
    video_play_context: VideoPlayContext,
    runtime_handle: Handle,
    pub is_running: bool,
}
impl PresentDataManager {
    pub fn new(
        runtime_handle: Handle,
        cancellation_token: Arc<CancellationToken>,
        audio_play_context: AudioPlayContext,
        video_play_context: VideoPlayContext,
    ) -> Self {
        let is_running = false;
        Self {
            audio_thread_handle: None,
            video_thread_handle: None,
            runtime_handle,
            is_running,
            audio_play_context,
            video_play_context,
            cancellation_token,
        }
    }
    async fn execute_audio_task(audio_play_context: AudioPlayContext) {
        let mut audio_cur_ts = None;
        while !audio_play_context.cancellation_token.is_cancelled() {
            /*
            add audio frame data to the audio player
             */
            if !audio_play_context
                .pause_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                if is_play_end(
                    &audio_play_context.live_mode,
                    &audio_play_context.audio_frame_receiver,
                    &audio_play_context.video_frame_receiver,
                    &audio_play_context.demux_eof_flag,
                ) {
                    audio_play_context
                        .pause_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if audio_play_context.audio_player.len() < 8 {
                    let mainstream = {
                        let tiny_decoder = audio_play_context.tiny_decoder.read().await;
                        tiny_decoder.main_stream.clone()
                    };
                    if let MainStream::Audio = &mainstream {
                        if audio_play_context.audio_frame_receiver.len() < 5 {
                            audio_play_context.audio_decode_thread_notify.notify_one();
                        }
                        if let Some(Ok(audio_frame)) = audio_play_context
                            .audio_frame_receiver
                            .recv_async()
                            .with_cancellation_token(&audio_play_context.cancellation_token)
                            .await
                            && let Some(pts) = audio_frame.pts()
                        {
                            audio_cur_ts = Some(pts);
                            if let Err(e) = audio_play_context
                                .audio_player
                                .append_source_data(audio_frame.clone())
                                .await
                            {
                                warn!("{}", e);
                            }
                            let used_model = audio_play_context.used_model.read().await;
                            let used_model_ref = &*used_model;
                            if UsedModel::None != *used_model_ref
                                && let Err(e) = audio_play_context
                                    .transcriber
                                    .write()
                                    .await
                                    .push_audio_frame(audio_frame)
                                    .await
                            {
                                warn!("transcribe err:{:?}", e);
                            }
                        }
                    }

                    PresentDataManager::update_current_timestamp(
                        audio_play_context.current_main_stream_timestamp.clone(),
                        audio_cur_ts,
                        mainstream,
                        audio_play_context.current_video_timestamp.clone(),
                    )
                    .await;
                }
            } else {
                audio_play_context.play_tasks_notify.notified().await;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    async fn execute_video_task(video_play_context: VideoPlayContext) {
        let mut change_instant = Instant::now();
        while !video_play_context.cancellation_token.is_cancelled() {
            if !video_play_context
                .pause_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                let (main_stream, audio_time_base, video_time_base) = {
                    let tiny_decoder = video_play_context.tiny_decoder.read().await;
                    (
                        tiny_decoder.main_stream.clone(),
                        tiny_decoder.audio_time_base,
                        tiny_decoder.video_time_base,
                    )
                };
                if is_play_end(
                    &video_play_context.live_mode,
                    &video_play_context.audio_frame_receiver,
                    &video_play_context.video_frame_receiver,
                    &video_play_context.demux_eof_flag,
                ) {
                    video_play_context
                        .pause_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if PresentDataManager::should_video_chase_audio(
                    main_stream.clone(),
                    audio_time_base,
                    video_time_base,
                    video_play_context.current_main_stream_timestamp.clone(),
                    video_play_context.current_video_timestamp.clone(),
                )
                .await
                {
                    let ins_now = Instant::now();
                    if video_play_context.video_frame_receiver.len() < 10 {
                        video_play_context.video_decode_thread_notify.notify_one();
                    }
                    let frame_result = match &main_stream {
                        MainStream::Video => {
                            if ins_now.checked_duration_since(change_instant).is_some() {
                                if let Some(Ok(frame)) = video_play_context
                                    .video_frame_receiver
                                    .recv_async()
                                    .with_cancellation_token(&video_play_context.cancellation_token)
                                    .await
                                {
                                    if let Some(f_pts) = frame.pts() {
                                        let cur_pts = video_play_context
                                            .current_video_timestamp
                                            .load(std::sync::atomic::Ordering::Relaxed);

                                        if f_pts > 0
                                            && ((f_pts - cur_pts)
                                                * 1000
                                                * video_time_base.numerator() as i64
                                                / video_time_base.denominator() as i64)
                                                < 1000
                                        {
                                            if let Some(ins) =
                                                change_instant.checked_add(Duration::from_millis(
                                                    ((f_pts - cur_pts)
                                                        * 1000
                                                        * video_time_base.numerator() as i64
                                                        / video_time_base.denominator() as i64)
                                                        as u64,
                                                ))
                                            {
                                                change_instant = ins;
                                            }
                                        } else {
                                            change_instant = ins_now;
                                        }
                                        video_play_context
                                            .current_video_timestamp
                                            .store(f_pts, std::sync::atomic::Ordering::Release);
                                        Ok(frame)
                                    } else {
                                        Err(anyhow::Error::msg("video frame pts is none"))
                                    }
                                } else {
                                    Err(anyhow::Error::msg("try video frame failed"))
                                }
                            } else {
                                Err(anyhow::Error::msg("video wait for its present time"))
                            }
                        }
                        MainStream::Audio => {
                            if let Some(Ok(frame)) = video_play_context
                                .video_frame_receiver
                                .recv_async()
                                .with_cancellation_token(&video_play_context.cancellation_token)
                                .await
                            {
                                if let Some(pts) = frame.pts() {
                                    video_play_context
                                        .current_video_timestamp
                                        .store(pts, std::sync::atomic::Ordering::Release);
                                }
                                Ok(frame)
                            } else {
                                Err(anyhow::Error::msg("try video frame failed"))
                            }
                        }
                    };
                    if let Ok(frame) = frame_result {
                        let mut transcoder = video_play_context.transcoder.write().await;

                        if let Err(e) = transcoder
                            .render_video(video_play_context.video_texture.clone(), frame)
                            .await
                        {
                            warn!("{}", e);
                        }
                    }
                }
            } else {
                video_play_context.play_tasks_notify.notified().await;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    async fn update_current_timestamp(
        main_stream_current_timestamp: Arc<AtomicI64>,
        audio_pts: Option<i64>,
        main_stream: MainStream,
        current_video_timestamp: Arc<AtomicI64>,
    ) {
        /*
        add audio frame data to the audio player
         */
        match main_stream {
            MainStream::Audio => {
                if let Some(pts) = audio_pts {
                    // info!("store main  timestamp:{}",pts);
                    main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Release);
                }
            }
            MainStream::Video => {
                let pts = current_video_timestamp.load(std::sync::atomic::Ordering::Relaxed);
                main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Release);
            }
        };
    }
    /// if video time-audio time is too high(more than 1 second),default return true
    async fn should_video_chase_audio(
        main_stream: MainStream,
        audio_time_base: Rational,
        video_time_base: Rational,
        main_stream_current_timestamp: Arc<AtomicI64>,
        current_video_timestamp: Arc<AtomicI64>,
    ) -> bool {
        if let MainStream::Video = &main_stream {
            return true;
        }
        let current_video_timestamp =
            current_video_timestamp.load(std::sync::atomic::Ordering::Acquire);

        let timestamp = main_stream_current_timestamp.load(std::sync::atomic::Ordering::Acquire);
        // info!("main ts:{},v_ts:{}", timestamp, current_video_timestamp);
        let v_time = current_video_timestamp * 1000 * video_time_base.numerator() as i64
            / video_time_base.denominator() as i64;
        let a_time = timestamp * 1000 * audio_time_base.numerator() as i64
            / audio_time_base.denominator() as i64;
        if a_time > v_time {
            return true;
        }

        false
    }
    pub async fn cancel_present_tasks(&mut self) -> PlayerResult<()> {
        self.cancellation_token.cancel();
        Self::join_tasks(
            self.runtime_handle.clone(),
            self.audio_thread_handle
                .take()
                .context("get audio_thread_handle err")?,
            self.video_thread_handle
                .take()
                .context("get audio_thread_handle err")?,
        );
        self.is_running = false;
        Ok(())
    }
    pub fn spawn_present_tasks(&mut self) {
        self.cancellation_token = Arc::new(CancellationToken::new());
        self.audio_play_context.cancellation_token = self.cancellation_token.clone();
        self.audio_thread_handle = Some(
            self.runtime_handle
                .spawn(Self::execute_audio_task(self.audio_play_context.clone())),
        );
        self.video_play_context.cancellation_token = self.cancellation_token.clone();
        self.video_thread_handle = Some(
            self.runtime_handle
                .spawn(Self::execute_video_task(self.video_play_context.clone())),
        );
        self.is_running = true;
    }
    fn join_tasks(
        runtime: Handle,
        audio_task_join_handle: JoinHandle<()>,
        video_task_join_handle: JoinHandle<()>,
    ) {
        runtime.spawn(async move {
            audio_task_join_handle.await?;
            video_task_join_handle.await?;
            info!("audio task and video task exit gracefully!");
            PlayerResult::Ok(())
        });
    }
}
impl Drop for PresentDataManager {
    fn drop(&mut self) {
        if self.is_running {
            self.cancellation_token.cancel();
        }
    }
}
fn is_play_end(
    live_mode: &AtomicBool,
    audio_frame_receiver: &Receiver<Audio>,
    video_frame_receiver: &Receiver<Video>,
    demux_eof_flag: &AtomicBool,
) -> bool {
    !live_mode.load(std::sync::atomic::Ordering::Relaxed)
        && demux_eof_flag.load(std::sync::atomic::Ordering::Relaxed)
        && audio_frame_receiver.is_empty()
        && video_frame_receiver.is_empty()
}
#[derive(Clone, TypedBuilder)]
pub struct AudioPlayContext {
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    used_model: Arc<RwLock<UsedModel>>,
    transcriber: Arc<RwLock<Transcriber>>,
    audio_player: Arc<AudioPlayer>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,
    pause_flag: Arc<AtomicBool>,

    audio_frame_receiver: Receiver<Audio>,
    video_frame_receiver: Receiver<Video>,
    audio_decode_thread_notify: Arc<Notify>,
    cancellation_token: Arc<CancellationToken>,
    play_tasks_notify: Arc<Notify>,
    demux_eof_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
}
#[derive(Clone, TypedBuilder)]
pub struct VideoPlayContext {
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,

    video_texture: Arc<RwLock<Texture>>,
    pause_flag: Arc<AtomicBool>,
    transcoder: Arc<RwLock<Transcoder>>,
    audio_frame_receiver: Receiver<Audio>,
    video_frame_receiver: Receiver<Video>,

    video_decode_thread_notify: Arc<Notify>,
    cancellation_token: Arc<CancellationToken>,
    play_tasks_notify: Arc<Notify>,
    demux_eof_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
}
