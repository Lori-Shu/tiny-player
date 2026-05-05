use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use derive_builder::Builder;
use eframe::wgpu::Texture;
use ffmpeg_the_third::{
    Rational,
    frame::{Audio, Video},
};
use flume::Receiver;
use rodio::Player;
use tokio::{
    runtime::Handle,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};
use tracing::warn;

use crate::{
    PlayerResult,
    audio_play::AudioPlayer,
    decode::{MainStream, TinyDecoder},
    gpu_post_process::ColorSpaceConverter,
};

pub struct PresentDataManager {
    audio_thread_handle: Option<JoinHandle<()>>,
    video_thread_handle: Option<JoinHandle<()>>,
    data_manage_context: DataManageContext,
}
impl PresentDataManager {
    pub fn new(data_manage_context: DataManageContext) -> Self {
        Self {
            audio_thread_handle: None,
            video_thread_handle: None,
            data_manage_context,
        }
    }
    async fn play_audio_task(data_manage_context: DataManageContext) {
        let mut audio_cur_ts = None;
        loop {
            /*
            add audio frame data to the audio player
             */
            if !data_manage_context
                .pause_flag
                .load(std::sync::atomic::Ordering::Acquire)
                && data_manage_context.audio_sink.len() < 10
            {
                let mainstream = {
                    let tiny_decoder = data_manage_context.tiny_decoder.read().await;
                    tiny_decoder.main_stream.clone()
                };
                if let MainStream::Audio = &mainstream {
                    if data_manage_context.audio_frame_receiver.len() < 5 {
                        data_manage_context.audio_decode_thread_notify.notify_one();
                    }
                    if let Ok(audio_frame) =
                        data_manage_context.audio_frame_receiver.recv_async().await
                    {
                        if let Some(pts) = audio_frame.pts() {
                            audio_cur_ts = Some(pts);
                            if let Err(e) = AudioPlayer::play_raw_data_from_audio_frame(
                                &data_manage_context.audio_sink,
                                audio_frame.clone(),
                            )
                            .await
                            {
                                warn!("{}", e);
                            }
                            // let used_model = data_manage_context.used_model.read().await;
                            // let used_model_ref = &*used_model;
                            // if UsedModel::Empty != *used_model_ref {
                            //     let mut ai_subtitle = data_manage_context.ai_subtitle.write().await;
                            //     let used_model = used_model_ref.clone();
                            //     ai_subtitle.push_frame_data(audio_frame, used_model).await;
                            // }
                        }
                    }
                }

                PresentDataManager::update_current_timestamp(
                    data_manage_context.main_stream_current_timestamp.clone(),
                    audio_cur_ts,
                    mainstream,
                    data_manage_context.current_video_timestamp.clone(),
                )
                .await;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    async fn play_video_task(data_manage_context: DataManageContext) {
        let mut change_instant = Instant::now();
        loop {
            if !data_manage_context
                .pause_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                let (main_stream, audio_time_base, video_time_base) = {
                    let tiny_decoder = data_manage_context.tiny_decoder.read().await;
                    (
                        tiny_decoder.main_stream.clone(),
                        tiny_decoder.audio_time_base,
                        tiny_decoder.video_time_base,
                    )
                };

                if PresentDataManager::should_video_catch_audio(
                    main_stream.clone(),
                    audio_time_base,
                    video_time_base,
                    data_manage_context.main_stream_current_timestamp.clone(),
                    data_manage_context.current_video_timestamp.clone(),
                )
                .await
                {
                    let ins_now = Instant::now();
                    if data_manage_context.video_frame_receiver.len() < 10 {
                        data_manage_context.video_decode_thread_notify.notify_one();
                    }
                    let frame_result = match &main_stream {
                        MainStream::Video => {
                            if ins_now.checked_duration_since(change_instant).is_some() {
                                if let Ok(frame) =
                                    data_manage_context.video_frame_receiver.recv_async().await
                                {
                                    if let Some(f_pts) = frame.pts() {
                                        let cur_pts = data_manage_context
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
                                        data_manage_context
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
                            if let Ok(frame) =
                                data_manage_context.video_frame_receiver.recv_async().await
                            {
                                if let Some(pts) = frame.pts() {
                                    data_manage_context
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
                        let mut color_space_converter =
                            data_manage_context.color_space_converter.write().await;

                        if let Err(e) = color_space_converter
                            .render_video(data_manage_context.video_texture.clone(), frame)
                            .await
                        {
                            warn!("{}", e);
                        }
                    }
                }
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
    async fn should_video_catch_audio(
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
        let time_dur = a_time - v_time;
        let time_dur_abs = time_dur.abs();
        if a_time > v_time || time_dur_abs > 10000 {
            return true;
        }

        false
    }
    pub fn stop_present_tasks(&self) -> PlayerResult<()> {
        let audio_task_join_handle = self
            .audio_thread_handle
            .as_ref()
            .context("no audio play task running")?;
        audio_task_join_handle.abort();
        let video_task_join_handle = self
            .video_thread_handle
            .as_ref()
            .context("no video play task running")?;
        video_task_join_handle.abort();
        Ok(())
    }
    pub fn start_present_tasks(&mut self) {
        let runtime_handle = self.data_manage_context.runtime_handle.clone();
        self.audio_thread_handle =
            Some(runtime_handle.spawn(Self::play_audio_task(self.data_manage_context.clone())));

        self.video_thread_handle =
            Some(runtime_handle.spawn(Self::play_video_task(self.data_manage_context.clone())));
    }
}
#[derive(Builder, Clone)]
pub struct DataManageContext {
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    // used_model: Arc<RwLock<UsedModel>>,
    // ai_subtitle: Arc<RwLock<AISubTitle>>,
    audio_sink: Arc<Player>,
    main_stream_current_timestamp: Arc<AtomicI64>,
    runtime_handle: Handle,
    current_video_timestamp: Arc<AtomicI64>,
    video_texture: Arc<RwLock<Texture>>,
    pause_flag: Arc<AtomicBool>,
    color_space_converter: Arc<RwLock<ColorSpaceConverter>>,
    audio_frame_receiver: Receiver<Audio>,
    video_frame_receiver: Receiver<Video>,
    audio_decode_thread_notify: Arc<Notify>,
    video_decode_thread_notify: Arc<Notify>,
}
