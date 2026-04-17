use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
    time::{Duration, Instant},
};

use derive_builder::Builder;
use ffmpeg_the_third::Rational;
use rodio::Player;
use tokio::{runtime::Handle, sync::RwLock, task::JoinHandle, time::sleep};
use tracing::warn;

use crate::{
    appui::VideoTextureWithId,
    audio_play::AudioPlayer,
    decode::{MainStream, TinyDecoder},
};

pub struct PresentDataManager {
    _audio_thread_handle: JoinHandle<()>,
    _video_thread_handle: JoinHandle<()>,
}
impl PresentDataManager {
    pub fn new(data_manage_context: DataManageContext) -> Self {
        let runtime_handle = data_manage_context.runtime_handle.clone();
        let _audio_thread_handle = runtime_handle.spawn(PresentDataManager::play_audio_task(
            data_manage_context.clone(),
        ));
        let _video_thread_handle =
            runtime_handle.spawn(PresentDataManager::play_video_task(data_manage_context));
        Self {
            _audio_thread_handle,
            _video_thread_handle,
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
                    let mut tiny_decoder = data_manage_context.tiny_decoder.write().await;
                    if let MainStream::Audio = tiny_decoder.main_stream() {
                        if let Some(audio_frame) = tiny_decoder.pull_one_audio_play_frame().await {
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

                    tiny_decoder.main_stream().clone()
                };

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
            let (main_stream, audio_time_base, video_time_base) = {
                let tiny_decoder = data_manage_context.tiny_decoder.read().await;
                (
                    tiny_decoder.main_stream().clone(),
                    *tiny_decoder.audio_time_base(),
                    *tiny_decoder.video_time_base(),
                )
            };
            let frame_result = if PresentDataManager::should_video_catch_audio(
                main_stream.clone(),
                audio_time_base,
                video_time_base,
                data_manage_context.main_stream_current_timestamp.clone(),
                data_manage_context.current_video_timestamp.clone(),
            )
            .await
            {
                let mut tiny_decoder = data_manage_context.tiny_decoder.write().await;
                let ins_now = Instant::now();
                if let Some(frame) = tiny_decoder.pull_one_video_play_frame().await {
                    let main_stream = tiny_decoder.main_stream();
                    let time_base = tiny_decoder.video_time_base();
                    if let MainStream::Video = main_stream {
                        if ins_now - change_instant > Duration::from_millis(0) {
                            if let Some(f_pts) = frame.pts() {
                                let cur_pts = data_manage_context
                                    .current_video_timestamp
                                    .load(std::sync::atomic::Ordering::Relaxed);

                                if f_pts > 0
                                    && cur_pts > 0
                                    && ((f_pts - cur_pts) * 1000 * (*time_base).numerator() as i64
                                        / (*time_base).denominator() as i64)
                                        < 1000
                                {
                                    if let Some(ins) =
                                        change_instant.checked_add(Duration::from_millis(
                                            ((f_pts - cur_pts)
                                                * 1000
                                                * (*time_base).numerator() as i64
                                                / (*time_base).denominator() as i64)
                                                as u64,
                                        ))
                                    {
                                        change_instant = ins;
                                    }
                                } else {
                                    change_instant = ins_now;
                                }
                            }
                        }
                    } else if let MainStream::Audio = main_stream {
                        change_instant = ins_now;
                    }
                    if let Some(pts) = frame.pts() {
                        data_manage_context
                            .current_video_timestamp
                            .store(pts, std::sync::atomic::Ordering::Release);
                    }
                    Ok(frame)
                } else {
                    Err(anyhow::Error::msg("no video frame"))
                }
            } else {
                Err(anyhow::Error::msg("no video frame"))
            };
            if let Ok(frame) = frame_result {
                let tiny_decoder = data_manage_context.tiny_decoder.read().await;
                if let Err(e) = tiny_decoder
                    .render_video_frame(data_manage_context.video_texture_with_id.clone(), frame)
                    .await
                {
                    warn!("{}", e);
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

        if let MainStream::Audio = main_stream {
            if let Some(pts) = audio_pts {
                // info!("store main  timestamp:{}",pts);
                main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Release);
            }
        } else if let MainStream::Video = main_stream {
            let pts = current_video_timestamp.load(std::sync::atomic::Ordering::Relaxed);
            main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Release);
        }
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
    video_texture_with_id: Arc<RwLock<VideoTextureWithId>>,
    pause_flag: Arc<AtomicBool>,
}
