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
use crossfire::{AsyncRx, MAsyncTx, spsc::Array};
use ffmpeg_the_third::{
    Rational,
    frame::{Audio, Video},
};
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
    post_process::Transcoder,
    whispercpp_transcriber::{AtomicTargetLanguage, TargetLanguage, Transcriber},
};
use media_engine::{FrameReceivers, MediaEngine};
pub const PLAY_SAMPLE_RATE: u32 = 48000;
pub struct MediaDataManager {
    audio_thread_handle: Option<JoinHandle<()>>,
    video_thread_handle: Option<JoinHandle<()>>,
    cancellation_token: Arc<CancellationToken>,
    tasks_notify: Arc<Notify>,
    audio_play_context: Arc<AudioPlayContext>,
    video_play_context: Arc<VideoPlayContext>,
    runtime_handle: Handle,
    pub is_running: bool,
}
impl MediaDataManager {
    pub fn new(
        runtime_handle: Handle,
        cancellation_token: Arc<CancellationToken>,
        audio_play_context: AudioPlayContext,
        video_play_context: VideoPlayContext,
    ) -> Self {
        let is_running = false;
        let tasks_notify = audio_play_context.play_tasks_notify.clone();
        Self {
            audio_thread_handle: None,
            video_thread_handle: None,
            runtime_handle,
            is_running,
            audio_play_context: Arc::new(audio_play_context),
            video_play_context: Arc::new(video_play_context),
            cancellation_token,
            tasks_notify,
        }
    }

    pub async fn cancel_present_tasks(&mut self) -> PlayerResult<()> {
        self.cancellation_token.cancel();
        self.tasks_notify.notify_waiters();
        let audio_thread_handle = self.audio_thread_handle.take();
        let video_thread_handle = self.video_thread_handle.take();
        self.join_tasks(audio_thread_handle, video_thread_handle)
            .await?;
        self.is_running = false;
        Ok(())
    }
    pub async fn spawn_present_tasks(&mut self, frame_receivers: FrameReceivers) {
        self.cancellation_token = Arc::new(CancellationToken::new());
        {
            let media_source_info = self
                .audio_play_context
                .media_engine
                .media_source_info()
                .await;
            if let Some(media_source_info) = &*media_source_info {
                if media_source_info.stream_existence_flags.audio {
                    let cancellation_token = self.cancellation_token.clone();
                    let audio_play_context = self.audio_play_context.clone();
                    self.audio_thread_handle = Some(
                        self.runtime_handle.spawn(
                            audio_play_context
                                .into_future(cancellation_token, frame_receivers.audio_frame_recv),
                        ),
                    );
                }
                if media_source_info.stream_existence_flags.video {
                    let cancellation_token = self.cancellation_token.clone();
                    let video_play_context = self.video_play_context.clone();
                    self.video_thread_handle = Some(
                        self.runtime_handle.spawn(
                            video_play_context
                                .into_future(cancellation_token, frame_receivers.video_frame_recv),
                        ),
                    );
                }
            }
            self.is_running = true;
        }
    }
    async fn join_tasks(
        &self,
        audio_task_join_handle: Option<JoinHandle<()>>,
        video_task_join_handle: Option<JoinHandle<()>>,
    ) -> PlayerResult<()> {
        if let Some(audio_task_join_handle) = audio_task_join_handle {
            info!("joining audio task!");
            audio_task_join_handle.await?;
        }
        if let Some(video_task_join_handle) = video_task_join_handle {
            info!("joining video task!");
            video_task_join_handle.await?;
        }
        Ok(())
    }
}
impl Drop for MediaDataManager {
    fn drop(&mut self) {
        if self.is_running {
            self.cancellation_token.cancel();
            self.tasks_notify.notify_waiters();
        }
    }
}

#[derive(TypedBuilder)]
pub struct AudioPlayContext {
    media_engine: Arc<MediaEngine>,
    target_language: Arc<AtomicTargetLanguage>,
    transcriber: Arc<RwLock<Transcriber>>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    pause_flag: Arc<AtomicBool>,
    play_tasks_notify: Arc<Notify>,
    demux_eof_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
    transcoder: Arc<RwLock<Transcoder>>,
    mel_bars_sender: MAsyncTx<crossfire::mpsc::Array<Vec<f32>>>,
    audio_playback_sender: MAsyncTx<crossfire::mpsc::Array<Audio>>,
}
impl AudioPlayContext {
    async fn into_future(
        self: Arc<Self>,
        cancellation_token: Arc<CancellationToken>,
        audio_frame_receiver: AsyncRx<Array<Audio>>,
    ) {
        if let Ok(mut frequency_analyzer) = FrequencyAnalyzer::new() {
            while !cancellation_token.is_cancelled() {
                /*
                add audio frame data to the audio player
                 */
                if !self.pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    if !self.live_mode.load(std::sync::atomic::Ordering::Relaxed)
                        && self
                            .demux_eof_flag
                            .load(std::sync::atomic::Ordering::Relaxed)
                        && audio_frame_receiver.is_empty()
                    {
                        self.pause_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    let video_flag = {
                        let media_source_info = self.media_engine.media_source_info().await;
                        if let Some(media_source_info) = &*media_source_info {
                            media_source_info.stream_existence_flags.video
                        } else {
                            false
                        }
                    };
                    if let Some(Ok(audio_frame)) = audio_frame_receiver
                        .recv()
                        .with_cancellation_token(&cancellation_token)
                        .await
                    {
                        if !video_flag {
                            let transcoder = self.transcoder.read().await;
                            transcoder.repaint_ui().await;
                        }
                        if TargetLanguage::None != self.target_language.load()
                            && let Err(e) = self
                                .transcriber
                                .write()
                                .await
                                .push_audio_frame(&audio_frame)
                                .await
                        {
                            warn!("transcribe err:{:?}", e);
                        }
                        if !video_flag
                            && let Ok(items) = frequency_analyzer.process_frame(&audio_frame).await
                        {
                            for i in items {
                                if let Err(e) = self.mel_bars_sender.send(i).await {
                                    warn!("send bars err:{:?}", e);
                                }
                            }
                        }
                        if let Some(pts) = audio_frame.pts() {
                            self.current_main_stream_timestamp
                                .store(pts, std::sync::atomic::Ordering::Relaxed);
                        }
                        if let Err(e) = self.audio_playback_sender.send(audio_frame).await {
                            warn!("{}", e);
                        }
                    }
                } else {
                    self.play_tasks_notify.notified().await;
                }
            }
        }
    }
}
#[derive(TypedBuilder)]
pub struct VideoPlayContext {
    media_engine: Arc<MediaEngine>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,
    pause_flag: Arc<AtomicBool>,
    transcoder: Arc<RwLock<Transcoder>>,
    play_tasks_notify: Arc<Notify>,
    demux_eof_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
}
impl VideoPlayContext {
    async fn into_future(
        self: Arc<Self>,
        cancellation_token: Arc<CancellationToken>,
        video_frame_receiver: AsyncRx<Array<Video>>,
    ) {
        let mut change_instant = Instant::now();
        while !cancellation_token.is_cancelled() {
            if !self.pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some((audio_existence_flag, audio_time_base, video_time_base)) = {
                    let media_source_info = self.media_engine.media_source_info().await;
                    media_source_info.as_ref().map(|info| {
                        (
                            info.stream_existence_flags.audio,
                            info.audio_time_base,
                            info.video_time_base,
                        )
                    })
                } {
                    if !self.live_mode.load(std::sync::atomic::Ordering::Relaxed)
                        && self
                            .demux_eof_flag
                            .load(std::sync::atomic::Ordering::Relaxed)
                        && video_frame_receiver.is_empty()
                    {
                        self.pause_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    if self
                        .should_video_chase_audio(
                            &audio_existence_flag,
                            &audio_time_base,
                            &video_time_base,
                        )
                        .await
                    {
                        let frame_result = if !audio_existence_flag {
                            let ins_now = Instant::now();
                            if ins_now.checked_duration_since(change_instant).is_some() {
                                if let Some(Ok(frame)) = video_frame_receiver
                                    .recv()
                                    .with_cancellation_token(&cancellation_token)
                                    .await
                                {
                                    self.sync_single_video_stream(
                                        &mut change_instant,
                                        &video_time_base,
                                        &frame,
                                        &ins_now,
                                    )
                                    .await;
                                    Ok(frame)
                                } else {
                                    Err(anyhow::Error::msg("try video frame failed"))
                                }
                            } else {
                                Err(anyhow::Error::msg("video wait for its present time"))
                            }
                        } else {
                            if let Some(Ok(frame)) = video_frame_receiver
                                .recv()
                                .with_cancellation_token(&cancellation_token)
                                .await
                            {
                                if let Some(pts) = frame.pts() {
                                    self.current_video_timestamp
                                        .store(pts, std::sync::atomic::Ordering::Relaxed);
                                }
                                Ok(frame)
                            } else {
                                Err(anyhow::Error::msg("try video frame failed"))
                            }
                        };
                        match frame_result {
                            Ok(frame) => {
                                let mut transcoder = self.transcoder.write().await;

                                if let Err(e) = transcoder.render_video(frame).await {
                                    warn!("{}", e);
                                }
                                transcoder.repaint_ui().await;
                            }
                            Err(e) => {
                                warn!("{}", e);
                            }
                        }
                    } else {
                        sleep(Duration::from_millis(8)).await;
                    }
                }
            } else {
                self.play_tasks_notify.notified().await;
            }
        }
    }
    async fn sync_single_video_stream(
        &self,
        change_instant: &mut Instant,
        video_time_base: &Rational,
        frame: &Video,
        ins_now: &Instant,
    ) {
        if let Some(f_pts) = frame.pts() {
            let cur_pts = self
                .current_video_timestamp
                .load(std::sync::atomic::Ordering::Relaxed);

            if f_pts > 0
                && ((f_pts - cur_pts) * 1000 * video_time_base.numerator() as i64
                    / video_time_base.denominator() as i64)
                    < 1000
            {
                if let Some(ins) = change_instant.checked_add(Duration::from_millis(
                    ((f_pts - cur_pts) * 1000 * video_time_base.numerator() as i64
                        / video_time_base.denominator() as i64) as u64,
                )) {
                    *change_instant = ins;
                }
            } else {
                *change_instant = *ins_now;
            }
            self.current_video_timestamp
                .store(f_pts, std::sync::atomic::Ordering::Relaxed);
            self.current_main_stream_timestamp
                .store(f_pts, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// if video time-audio time is too high(more than 1 second),default return true
    async fn should_video_chase_audio(
        &self,
        audio_stream_flag: &bool,
        audio_time_base: &Rational,
        video_time_base: &Rational,
    ) -> bool {
        if !audio_stream_flag {
            return true;
        }
        let video_timestamp = self
            .current_video_timestamp
            .load(std::sync::atomic::Ordering::Relaxed);

        let timestamp = self
            .current_main_stream_timestamp
            .load(std::sync::atomic::Ordering::Relaxed);
        let v_time = video_timestamp * 1000 * video_time_base.numerator() as i64
            / video_time_base.denominator() as i64;
        let a_time = timestamp * 1000 * audio_time_base.numerator() as i64
            / audio_time_base.denominator() as i64;

        // auto heal. synchronizing logic
        {
            if v_time - a_time > 1000 {
                let heal_timestamp = (a_time + 500) * video_time_base.denominator() as i64
                    / video_time_base.numerator() as i64
                    / 1000;
                // info!("main time:{},v_time:{}", a_time, v_time);
                // info!(
                //     "main timestamp:{},v_timestamp:{}",
                //     timestamp, video_time_base
                // );
                // info!("heal_timestamp:{}", heal_timestamp);
                self.current_video_timestamp
                    .store(heal_timestamp, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
        }

        a_time > v_time
    }
}
const FFT_SIZE: usize = 1024;
const FILTERBANK_SIZE: u32 = 32;
struct FrequencyAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    buffer: VecDeque<f32>,
    mel_filterbank: [[f32; FFT_SIZE / 2 + 1]; FILTERBANK_SIZE as usize + 1],
    hann_window: [f32; FFT_SIZE],
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
        frame: &ffmpeg_the_third::frame::Audio,
    ) -> PlayerResult<Vec<Vec<f32>>> {
        let frame_bytes = &frame.data(0)
            [0..(size_of::<f32>() * frame.samples() * frame.ch_layout().channels() as usize)];
        self.buffer
            .extend(bytemuck::cast_slice::<_, f32>(frame_bytes));
        let mut fft_res = vec![];
        let mut complex_buffer = [Complex { re: 0.0, im: 0.0 }; FFT_SIZE];
        while self.buffer.len() >= FFT_SIZE {
            for c in &mut complex_buffer {
                *c = Complex {
                    re: self.buffer.pop_front().context("pop floating-point err")?,
                    im: 0.0,
                };
            }
            for (idx, sample) in complex_buffer.iter_mut().enumerate() {
                sample.re *= self.hann_window[idx];
            }
            self.fft.process(&mut complex_buffer);
            let real_powers = complex_buffer
                .iter()
                .take(FFT_SIZE / 2 + 1)
                .map(|i| i.norm_sqr())
                .collect::<Vec<f32>>();
            fft_res.push(real_powers);
        }
        let mut res = vec![];
        for fft_coe in &fft_res {
            let mut bar_nums = vec![];
            for filterbank in &self.mel_filterbank {
                let reduce_res = filterbank
                    .iter()
                    .enumerate()
                    .map(|(idx, mel)| *mel * fft_coe[idx])
                    .reduce(|i, j| i + j)
                    .context("reduce mel result err")?;
                bar_nums.push(reduce_res);
            }
            bar_nums
                .iter_mut()
                .for_each(|i| *i = 10.0 * (*i + 1e-8).log10());
            // info!("barnums:{:?}",bar_nums);
            res.push(bar_nums);
        }
        Ok(res)
    }
    fn construct_hann_window() -> [f32; FFT_SIZE] {
        let mut window = [0.0; FFT_SIZE];
        for (idx, item) in window.iter_mut().enumerate().take(FFT_SIZE) {
            *item = 0.5 * (1.0 - (2.0 * PI * idx as f32 / (FFT_SIZE - 1) as f32).cos());
        }
        window
    }
    fn construct_mel_filterbank()
    -> PlayerResult<[[f32; FFT_SIZE / 2 + 1]; FILTERBANK_SIZE as usize + 1]> {
        const F_MIN: u32 = 20;
        const F_MAX: u32 = PLAY_SAMPLE_RATE / 2;
        let mel_min = 2595.0 * (1.0 + F_MIN as f32 / 700.0).log10();
        let mel_max = 2595.0 * (1.0 + F_MAX as f32 / 700.0).log10();
        let step = (mel_max - mel_min) / FILTERBANK_SIZE as f32;
        let mut mel_points = [0.0; FILTERBANK_SIZE as usize + 1];
        let mut tmp = mel_min;
        for item in mel_points.iter_mut().take(FILTERBANK_SIZE as usize + 1) {
            *item = tmp;
            tmp += step;
        }
        mel_points
            .iter_mut()
            .for_each(|point| *point = 700.0 * (10.0_f32.powf(*point / 2595.0) - 1.0));
        let bins = mel_points
            .iter()
            .map(|f| ((FFT_SIZE as f32) * (*f) / PLAY_SAMPLE_RATE as f32) as usize)
            .collect::<Vec<usize>>();
        let mut mel_filterbank = [[0.0; FFT_SIZE / 2 + 1]; FILTERBANK_SIZE as usize + 1];
        for i in 0..(FILTERBANK_SIZE as usize + 1 - 2) {
            let l = bins[i];
            let c = bins[i + 1];
            let r = bins[i + 2];

            let mut hs = [0.0; FFT_SIZE / 2 + 1];
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
            mel_filterbank[i] = hs;
        }
        Ok(mel_filterbank)
    }
}
