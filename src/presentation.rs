//! The presentation module manages data synchronization and presentation
use std::{
    collections::VecDeque,
    f32::consts::PI,
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
use flume::{Receiver, Sender};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
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
    post_process::Transcoder,
    whispercpp_transcriber::{Transcriber, UsedModel},
};
use media_engine::MediaEngine;
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
        if let Ok(mut frequency_analyzer) = FrequencyAnalyzer::new() {
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
                        let audio_stream_flag = {
                            if let Ok(info) = audio_play_context.media_engine.media_source_info() {
                                info.stream_existence_flags.audio
                            } else {
                                return;
                            }
                        };
                        if audio_stream_flag {
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
                                {
                                    let transcoder = audio_play_context.transcoder.read().await;
                                    transcoder.repaint_ui().await;
                                }
                                let used_model = audio_play_context.used_model.read().await;
                                let used_model_ref = &*used_model;
                                if UsedModel::None != *used_model_ref
                                    && let Err(e) = audio_play_context
                                        .transcriber
                                        .write()
                                        .await
                                        .push_audio_frame(audio_frame.clone())
                                        .await
                                {
                                    warn!("transcribe err:{:?}", e);
                                }
                                if let Ok(items) =
                                    frequency_analyzer.process_frame(audio_frame).await
                                {
                                    for i in items {
                                        if let Err(e) =
                                            audio_play_context.mel_bars_sender.send_async(i).await
                                        {
                                            warn!("send bars err:{:?}", e);
                                        }
                                    }
                                }
                            }
                        }

                        PresentDataManager::update_current_timestamp(
                            audio_play_context.current_main_stream_timestamp.clone(),
                            audio_cur_ts,
                            audio_stream_flag,
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
    }
    async fn execute_video_task(video_play_context: VideoPlayContext) {
        let mut change_instant = Instant::now();
        while !video_play_context.cancellation_token.is_cancelled() {
            if !video_play_context
                .pause_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                let (audio_existence_flag, audio_time_base, video_time_base) = {
                    if let Ok(info) = video_play_context.media_engine.media_source_info() {
                        (
                            info.stream_existence_flags.audio,
                            info.audio_time_base,
                            info.video_time_base,
                        )
                    } else {
                        return;
                    }
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
                    audio_existence_flag,
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
                    let frame_result = if !audio_existence_flag {
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
                                        .store(f_pts, std::sync::atomic::Ordering::Relaxed);
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
                    } else {
                        if let Some(Ok(frame)) = video_play_context
                            .video_frame_receiver
                            .recv_async()
                            .with_cancellation_token(&video_play_context.cancellation_token)
                            .await
                        {
                            if let Some(pts) = frame.pts() {
                                video_play_context
                                    .current_video_timestamp
                                    .store(pts, std::sync::atomic::Ordering::Relaxed);
                            }
                            Ok(frame)
                        } else {
                            Err(anyhow::Error::msg("try video frame failed"))
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
        audio_existence_flag: bool,
        current_video_timestamp: Arc<AtomicI64>,
    ) {
        /*
        add audio frame data to the audio player
         */
        if audio_existence_flag {
            if let Some(pts) = audio_pts {
                // info!("store main  timestamp:{}",pts);
                main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Relaxed);
            }
        } else {
            let pts = current_video_timestamp.load(std::sync::atomic::Ordering::Relaxed);
            main_stream_current_timestamp.store(pts, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// if video time-audio time is too high(more than 1 second),default return true
    async fn should_video_chase_audio(
        audio_stream_flag: bool,
        audio_time_base: Rational,
        video_time_base: Rational,
        main_stream_current_timestamp: Arc<AtomicI64>,
        current_video_timestamp: Arc<AtomicI64>,
    ) -> bool {
        if !audio_stream_flag {
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
    media_engine: Arc<MediaEngine>,
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

    transcoder: Arc<RwLock<Transcoder>>,
    mel_bars_sender: Sender<Vec<f32>>,
}
#[derive(Clone, TypedBuilder)]
pub struct VideoPlayContext {
    media_engine: Arc<MediaEngine>,
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
const FFT_SIZE: usize = 1024;
struct FrequencyAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    buffer: VecDeque<f32>,
    mel_filterbank: Vec<Vec<f32>>,
    hann_window: Vec<f32>,
}
impl FrequencyAnalyzer {
    fn new() -> PlayerResult<Self> {
        let mut fft_planner = FftPlanner::<f32>::new();

        let fft = fft_planner.plan_fft(FFT_SIZE, rustfft::FftDirection::Forward);
        let mel_filterbank = Self::construct_mel_filterbank()?;
        let hann_window = Self::construct_hann_window();
        Ok(Self {
            fft,
            buffer: VecDeque::new(),
            mel_filterbank,
            hann_window,
        })
    }
    async fn process_frame(
        &mut self,
        frame: ffmpeg_the_third::frame::Audio,
    ) -> PlayerResult<Vec<Vec<f32>>> {
        let frame_bytes = &frame.data(0)
            [0..(size_of::<f32>() * frame.samples() * frame.ch_layout().channels() as usize)];
        self.buffer
            .extend(bytemuck::cast_slice::<_, f32>(frame_bytes));
        let mut fft_res = vec![];
        let mut buffer = vec![Complex { re: 0.0, im: 0.0 }; FFT_SIZE];
        while self.buffer.len() >= FFT_SIZE {
            for c in &mut buffer {
                *c = Complex {
                    re: self.buffer.pop_front().context("pop floating-point err")?,
                    im: 0.0,
                };
            }
            for (idx, sample) in buffer.iter_mut().enumerate() {
                sample.re *= self.hann_window[idx];
            }
            self.fft.process(&mut buffer);
            let real_powers = buffer
                .iter()
                .take(FFT_SIZE / 2 + 1)
                .map(|i| i.norm_sqr())
                .collect::<Vec<f32>>();
            fft_res.push(real_powers);
        }
        let mut res = vec![];
        for i in &fft_res {
            let mut bar_nums = vec![];
            for j in &self.mel_filterbank {
                let reduce_res = j
                    .iter()
                    .enumerate()
                    .map(|(idx, mel)| *mel * i[idx])
                    .reduce(|i, j| i + j)
                    .context("reduce mel result err")?;
                bar_nums.push(reduce_res);
            }
            bar_nums
                .iter_mut()
                .for_each(|i| *i = 10.0 * (*i + 1e-8).log10());
            res.push(bar_nums);
        }
        Ok(res)
    }
    fn construct_hann_window() -> Vec<f32> {
        (0..FFT_SIZE)
            .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / (FFT_SIZE - 1) as f32).cos()))
            .collect()
    }
    fn construct_mel_filterbank() -> PlayerResult<Vec<Vec<f32>>> {
        let filterbank_size = 32;
        let f_min = 20_u32;
        let f_max = PLAY_SAMPLE_RATE / 2;
        let mel_min = 2595.0 * ((1.0 + f_min as f32) / 700.0).log10();
        let mel_max = 2595.0 * ((1.0 + f_max as f32) / 700.0).log10();
        let step = (mel_max - mel_min) / (filterbank_size as f32 + 1.0);
        let mut mel_points = vec![mel_min];
        let mut tmp = mel_min;
        for _ in 0..(filterbank_size + 1) {
            tmp += step;
            mel_points.push(tmp);
        }
        mel_points
            .iter_mut()
            .for_each(|point| *point = 700.0 * (10.0_f32.powf(*point / 2595.0) - 1.0));
        let bins = mel_points
            .iter()
            .map(|f| ((FFT_SIZE as f32 + 1.0) * (*f) / PLAY_SAMPLE_RATE as f32) as usize)
            .collect::<Vec<usize>>();
        let mut mel_filterbank = vec![];
        for i in 0..filterbank_size as usize {
            let l = bins[i];
            let c = bins[i + 1];
            let r = bins[i + 2];

            let mut hs = vec![0.0; FFT_SIZE / 2 + 1];
            if c == l || r == c {
                continue;
            }
            for (idx, j) in hs.iter_mut().enumerate() {
                if idx >= l && idx <= c {
                    *j = ((idx - l) as f32) / (c - l) as f32;
                } else if idx > c && idx <= r {
                    *j = ((r - idx) as f32) / (r - c) as f32;
                }
            }
            mel_filterbank.push(hs);
        }
        Ok(mel_filterbank)
    }
}
