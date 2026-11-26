//! The whispercpp_transcriber adds
//! audio transcribing support for tiny-player
use std::{
    io::Cursor,
    process::{Child, Command},
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
    time::Instant,
};

use anyhow::Context;
use crossfire::{
    AsyncRx, MAsyncTx,
    mpsc::{Array, bounded_async},
};
use ffmpeg_the_third::{
    ChannelLayout,
    ffi::{
        AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, SwrContext, swr_alloc_set_opts2,
        swr_convert_frame, swr_free, swr_init,
    },
    format::Sample,
    frame::Audio,
};
use hound::{WavSpec, WavWriter};
use reqwest::Client;
use tokio::{runtime::Handle, sync::Notify};
use tokio_util::{future::FutureExt, sync::CancellationToken};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{CURRENT_EXE_PATH, PlayerResult, presentation::PLAY_SAMPLE_RATE};
/// this wrapper type should be protected manually to
/// keep memory safe in multi threads
/// means need to wrap an Arc and a Lock to use it in multi threads
pub struct ManualProtectedResampler(pub *mut SwrContext);
unsafe impl Send for ManualProtectedResampler {}
unsafe impl Sync for ManualProtectedResampler {}
const TRANSCRIBE_SAMPLE_RATE: u32 = 16000;
const THREE_SEC_BYTES_LEN: usize = (TRANSCRIBE_SAMPLE_RATE as usize) * 3 * size_of::<i16>();

/// Transcriber type which handles audio normalization and
/// communication with whisper server
pub struct Transcriber {
    audio_resampler: ManualProtectedResampler,
    audio_frame_bytes_sender: MAsyncTx<Array<Audio>>,
}
impl Transcriber {
    pub fn new(args: TranscriberArgs) -> PlayerResult<(Self, Child)> {
        let exe_path = if let Ok(exe_path) = CURRENT_EXE_PATH.as_ref() {
            exe_path.as_path()
        } else {
            return Err(anyhow::Error::msg("getting CURRENT_EXE_PATH failed"));
        };
        let exe_dir = exe_path.parent().context("get parent_dir err")?;
        let models_dir_path = exe_dir.join("models");
        let model_path = models_dir_path.join("ggml-base-q8_0.bin");
        let path_str = model_path.to_str().context("to str failed")?;
        // SAFETY:
        // This unsafe block is promised to be safe because
        // the ManualProtectedResampler is bound to be free
        // at the drop phase
        let resampler_ctx = unsafe {
            let mut swr_ctx = null_mut();
            let r = swr_alloc_set_opts2(
                &mut swr_ctx,
                &AV_CHANNEL_LAYOUT_MONO,
                ffmpeg_the_third::ffi::AVSampleFormat::S16,
                TRANSCRIBE_SAMPLE_RATE as i32,
                &AV_CHANNEL_LAYOUT_STEREO,
                ffmpeg_the_third::ffi::AVSampleFormat::FLT,
                PLAY_SAMPLE_RATE as i32,
                0,
                null_mut(),
            );
            if r < 0 {
                warn!("swr ctx create err");
                return Err(anyhow::Error::msg("swr ctx create err"));
            }
            let r = swr_init(swr_ctx);
            if r < 0 {
                warn!("swr init err");
                return Err(anyhow::Error::msg("swr init err"));
            }
            ManualProtectedResampler(swr_ctx)
        };
        const SPAWNING_PROCESS_ATTEMPT_TIMES: u32 = 10;
        let mut times = 0;
        let (child_process, server_url) = loop {
            let random_port = rand::random_range(10000..65535);
            let random_port_str = random_port.to_string();
            let mut whisper_command = Command::new("whisper-server.exe");
            whisper_command
                .arg("--language")
                .arg("auto")
                .arg("--model")
                .arg(path_str)
                .arg("--port")
                .arg(&random_port_str);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;

                const CREATE_NO_WINDOW: u32 = 0x08000000;
                whisper_command.creation_flags(CREATE_NO_WINDOW);
            }
            let spawn_result = whisper_command.spawn().map(|child_process| {
                let mut server_url = String::from("http://127.0.0.1:");
                server_url.push_str(&random_port_str);
                server_url.push_str("/inference");
                (child_process, server_url)
            });
            if spawn_result.is_ok() || times >= SPAWNING_PROCESS_ATTEMPT_TIMES {
                break spawn_result;
            }
            times += 1;
        }?;
        let transcribe_task_cancel_token = CancellationToken::new();
        let transcribe_task_notify_cloned = args.transcribe_task_notify.clone();
        let transcribe_task_cancel_token_cloned = transcribe_task_cancel_token.clone();
        let (audio_frame_bytes_sender, audio_frame_bytes_receiver) = bounded_async(64);
        let target_language = args.target_language.clone();
        let pause_flag = args.pause_flag.clone();
        let subtitle_sender = args.subtitle_sender;
        let transcribe_task = TranscribeTask::new(
            server_url,
            transcribe_task_cancel_token_cloned,
            transcribe_task_notify_cloned,
            target_language,
            pause_flag,
            subtitle_sender,
        );
        let _transcribe_task_handle = args
            .async_runtime
            .spawn(transcribe_task.into_future(audio_frame_bytes_receiver));
        Ok((
            Self {
                audio_resampler: resampler_ctx,
                audio_frame_bytes_sender,
            },
            child_process,
        ))
    }

    pub async fn push_audio_frame(&mut self, frame: &Audio) -> PlayerResult<()> {
        let mut transcribe_frame = Audio::empty();
        transcribe_frame.set_format(Sample::I16(ffmpeg_the_third::format::sample::Type::Packed));
        transcribe_frame.set_ch_layout(ChannelLayout::MONO);
        transcribe_frame.set_rate(TRANSCRIBE_SAMPLE_RATE);
        // SAFETY:
        // This unsafe block is promised to be safe because
        // the inner operations do not alloc extra memory
        unsafe {
            let err_num = swr_convert_frame(
                self.audio_resampler.0,
                transcribe_frame.as_mut_ptr(),
                frame.as_ptr(),
            );
            if err_num < 0 {
                let err_msg = format!("audio frame convert err: {}", err_num);
                warn!(err_msg);
                return Err(anyhow::Error::msg(err_msg));
            }
        }
        self.audio_frame_bytes_sender.send(transcribe_frame).await?;

        Ok(())
    }
}
impl Drop for Transcriber {
    fn drop(&mut self) {
        info!("start dropping Transcriber resources");
        // SAFETY:
        // This unsafe block is promised to be safe because
        // the inner operations do not alloc extra memory
        // but free the resampler memory
        unsafe {
            swr_free(&mut self.audio_resampler.0);
        }
    }
}
struct TranscribeTask {
    server_url: Box<str>,
    transcribe_task_cancel_token: CancellationToken,
    transcribe_task_notify: Arc<Notify>,
    target_language: Arc<AtomicTargetLanguage>,
    pause_flag: Arc<AtomicBool>,
    subtitle_sender: MAsyncTx<Array<String>>,
    buffer: Vec<u8>,
}
impl TranscribeTask {
    fn new(
        server_url: String,
        transcribe_task_cancel_token: CancellationToken,
        transcribe_task_notify: Arc<Notify>,
        target_language: Arc<AtomicTargetLanguage>,
        pause_flag: Arc<AtomicBool>,
        subtitle_sender: MAsyncTx<Array<String>>,
    ) -> Self {
        Self {
            server_url: server_url.into_boxed_str(),
            transcribe_task_cancel_token,
            transcribe_task_notify,
            target_language,
            pause_flag,
            subtitle_sender,
            buffer: vec![],
        }
    }
    async fn into_future(mut self, audio_frame_bytes_receiver: AsyncRx<Array<Audio>>) {
        let network_client = Client::new();
        let mut last_transcrib_instant = Instant::now();
        while !self.transcribe_task_cancel_token.is_cancelled() {
            let used_model = self.target_language.load();
            if !self.pause_flag.load(std::sync::atomic::Ordering::Relaxed)
                && TargetLanguage::None != used_model
            {
                if let Some(Ok(frame)) = audio_frame_bytes_receiver
                    .recv()
                    .with_cancellation_token(&self.transcribe_task_cancel_token)
                    .await
                {
                    self.buffer
                        .extend(&frame.data(0)[0..(frame.samples() * size_of::<i16>())]);
                }

                if last_transcrib_instant.elapsed().as_millis() > 200
                    && self.buffer.len() > TRANSCRIBE_SAMPLE_RATE as usize
                {
                    self.transcribe(&network_client, &self.buffer, &used_model)
                        .await;
                    if self.buffer.len() >= THREE_SEC_BYTES_LEN {
                        self.buffer.drain(0..THREE_SEC_BYTES_LEN);
                    }
                    last_transcrib_instant = Instant::now();
                }
            } else {
                self.transcribe_task_notify.notified().await;
                info!("transcribe task waked");
            }
        }
    }
    async fn transcribe(
        &self,
        network_client: &Client,
        contiguous_slice: &[u8],
        target_language: &TargetLanguage,
    ) {
        match self
            .send_request(network_client, contiguous_slice, target_language)
            .await
        {
            Ok(audio_script) => {
                for line in audio_script.lines() {
                    if let Err(e) = self.subtitle_sender.send(line.to_string()).await {
                        warn!("subtitle_sender err:{:?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("{}", e);
            }
        }
    }
    async fn package_wav_bytes(&self, pcm_data: &[u8]) -> PlayerResult<Vec<u8>> {
        let spec = WavSpec {
            channels: 1,
            sample_rate: TRANSCRIBE_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut cursor = Cursor::new(Vec::with_capacity(44 + pcm_data.len()));
        {
            let mut writer = WavWriter::new(&mut cursor, spec)?;

            for chunk in pcm_data.as_chunks::<2>().0 {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                writer.write_sample(sample)?;
            }
        }

        Ok(cursor.into_inner())
    }
    async fn send_request(
        &self,
        network_client: &Client,
        bytes: &[u8],
        target_language: &TargetLanguage,
    ) -> PlayerResult<String> {
        let model_str = match target_language {
            TargetLanguage::None => {
                return Err(anyhow::Error::msg(
                    "used_model should not be none in send_request",
                ));
            }
            TargetLanguage::English => "en",
            TargetLanguage::Chinese => "zh",
        };
        let wav_bytes_with_header = self.package_wav_bytes(bytes).await?;
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(wav_bytes_with_header)
                    .file_name("chunk.wav")
                    .mime_str("audio/wav")?,
            )
            .text("language", model_str)
            .text("response_format", "json");
        let audio_scripts = network_client
            .post(self.server_url.as_ref())
            .multipart(form)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?["text"]
            .as_str()
            .context("parse serde_json::Value to str err!")?
            .to_string();
        // info!("{}", audio_scripts);
        Ok(audio_scripts)
    }
}
impl Drop for TranscribeTask {
    fn drop(&mut self) {
        self.transcribe_task_cancel_token.cancel();
        self.transcribe_task_notify.notify_one();
    }
}
#[repr(u8)]
#[derive(Debug, PartialEq, Clone)]
pub enum TargetLanguage {
    None = 0,
    Chinese = 1,
    English = 2,
}

pub struct AtomicTargetLanguage {
    target_num: AtomicU8,
}
impl AtomicTargetLanguage {
    pub fn new() -> Self {
        Self {
            target_num: AtomicU8::new(0),
        }
    }
    pub fn load(&self) -> TargetLanguage {
        match self.target_num.load(std::sync::atomic::Ordering::Relaxed) {
            0 => TargetLanguage::None,
            1 => TargetLanguage::Chinese,
            2 => TargetLanguage::English,
            _ => {
                warn!("unexpected atomic number!");
                TargetLanguage::None
            }
        }
    }
    pub fn store(&self, language: TargetLanguage) {
        match language {
            TargetLanguage::None => self
                .target_num
                .store(0, std::sync::atomic::Ordering::Relaxed),
            TargetLanguage::Chinese => self
                .target_num
                .store(1, std::sync::atomic::Ordering::Relaxed),
            TargetLanguage::English => self
                .target_num
                .store(2, std::sync::atomic::Ordering::Relaxed),
        }
    }
}
#[derive(TypedBuilder)]
pub struct TranscriberArgs {
    async_runtime: Handle,
    subtitle_sender: MAsyncTx<Array<String>>,
    pause_flag: Arc<AtomicBool>,
    transcribe_task_notify: Arc<Notify>,
    target_language: Arc<AtomicTargetLanguage>,
}
