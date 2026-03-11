use std::{
    sync::{Arc, Condvar},
    time::{Duration, Instant},
};

use derive_builder::Builder;
use ffmpeg_the_third::frame::{Audio, Video};
use rodio::Sink;
use tokio::{
    runtime::Handle,
    sync::{Notify, RwLock, mpsc::UnboundedSender},
    task::JoinHandle,
};
use tracing::warn;

use crate::{
    ai_sub_title2::UsedModel,
    audio_play::AudioPlayer,
    decode::{MainStream, TinyDecoder},
};

pub struct PresentDataManager {
    _data_thread_handle: JoinHandle<()>,
}
impl PresentDataManager {
    pub fn new(data_manage_context: DataManageContext) -> Self {
        Self {
            _data_thread_handle: data_manage_context
                .runtime_handle
                .clone()
                .spawn(PresentDataManager::play_task(data_manage_context)),
        }
    }
    async fn play_task(data_manage_context: DataManageContext) {
        let mut change_instant = Instant::now();
        loop {
            /*
            add audio frame data to the audio player
             */
            {
                let mut audio_cur_ts = None;
                let mut tiny_decoder = data_manage_context.tiny_decoder.write().await;
                if let MainStream::Audio = tiny_decoder.main_stream() {
                    if let Some(audio_frame) = tiny_decoder.pull_one_audio_play_frame().await {
                        if let Some(pts) = audio_frame.pts() {
                            audio_cur_ts = Some(pts);
                            AudioPlayer::play_raw_data_from_audio_frame(
                                &data_manage_context.audio_sink,
                                audio_frame.clone(),
                            )
                            .await;
                            let used_model = data_manage_context.used_model.read().await;
                            let used_model_ref = &*used_model;
                            if UsedModel::Empty != *used_model_ref {
                                let used_model = used_model_ref.clone();
                                if let Err(e) = data_manage_context
                                    .frame_sender
                                    .send((audio_frame, used_model))
                                {
                                    warn!("send audio frame to subtitle thread failed!!!{}", e);
                                }
                                data_manage_context.transcribe_thread_condvar.notify_one();
                            }
                        }
                    }
                }
                if PresentDataManager::should_video_catch_audio(
                    &tiny_decoder,
                    data_manage_context.main_stream_current_timestamp.clone(),
                    data_manage_context.current_video_frame.clone(),
                )
                .await
                {
                    let ins_now = Instant::now();
                    if let Some(frame) = tiny_decoder.pull_one_video_play_frame().await {
                        let mut cur_frame = data_manage_context.current_video_frame.write().await;
                        let main_stream = tiny_decoder.main_stream();
                        if let MainStream::Video = main_stream {
                            if ins_now - change_instant > Duration::from_millis(0) {
                                if let Some(f_pts) = frame.pts() {
                                    if let Some(cur_pts) = cur_frame.pts() {
                                        let time_base = tiny_decoder.video_time_base();
                                        if f_pts > 0
                                            && cur_pts > 0
                                            && ((f_pts - cur_pts)
                                                * 1000
                                                * (*time_base).numerator() as i64
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
                            }
                        } else if let MainStream::Audio = main_stream {
                            change_instant = ins_now;
                        }
                        *cur_frame = frame;
                    }
                }
                PresentDataManager::update_current_timestamp(
                    data_manage_context.main_stream_current_timestamp.clone(),
                    audio_cur_ts,
                    &tiny_decoder,
                    data_manage_context.current_video_frame.clone(),
                )
                .await;
            }
            data_manage_context.data_thread_notify.notified().await;
        }
    }
    async fn update_current_timestamp(
        main_stream_current_timestamp: Arc<RwLock<i64>>,
        audio_pts: Option<i64>,
        tiny_decoder: &TinyDecoder,
        current_video_frame: Arc<RwLock<Video>>,
    ) {
        /*
        add audio frame data to the audio player
         */
        let main_stream = tiny_decoder.main_stream();
        let mut main_ts = main_stream_current_timestamp.write().await;
        if let MainStream::Audio = main_stream {
            if let Some(pts) = audio_pts {
                *main_ts = pts;
            }
        } else if let MainStream::Video = main_stream {
            let cur_video_frame = current_video_frame.read().await;

            if let Some(pts) = cur_video_frame.pts() {
                if pts > 0 {
                    *main_ts = pts;
                }
            }
        }
    }
    /// if video time-audio time is too high(more than 1 second),default return true
    async fn should_video_catch_audio(
        tiny_decoder: &TinyDecoder,
        main_stream_current_timestamp: Arc<RwLock<i64>>,
        current_video_frame: Arc<RwLock<Video>>,
    ) -> bool {
        if let MainStream::Video = tiny_decoder.main_stream() {
            return true;
        }
        let current_video_frame = current_video_frame.read().await;

        let timestamp = main_stream_current_timestamp.read().await;
        {
            let video_time_base = tiny_decoder.video_time_base();
            let audio_time_base = tiny_decoder.audio_time_base();
            if let Some(f_ts) = current_video_frame.pts() {
                let v_time = f_ts * 1000 * video_time_base.numerator() as i64
                    / video_time_base.denominator() as i64;
                let a_time = *timestamp * 1000 * audio_time_base.numerator() as i64
                    / audio_time_base.denominator() as i64;
                let time_dur = a_time - v_time;
                if time_dur > 100 {
                    return true;
                }
            } else {
                return true;
            }
        }

        false
    }
}
#[derive(Builder)]
pub struct DataManageContext {
    frame_sender: UnboundedSender<(Audio, UsedModel)>,
    transcribe_thread_condvar: Arc<Condvar>,
    data_thread_notify: Arc<Notify>,
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    used_model: Arc<RwLock<UsedModel>>,
    current_video_frame: Arc<RwLock<Video>>,
    audio_sink: Arc<Sink>,
    main_stream_current_timestamp: Arc<RwLock<i64>>,
    runtime_handle: Handle,
}
