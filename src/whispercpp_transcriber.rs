//! The whispercpp_transcriber adds
//! audio transcribing support for tiny-player
use std::{
    collections::VecDeque,
    io::Cursor,
    process::{Child, Command},
    ptr::null_mut,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
    time::Duration,
};

use anyhow::Context;
use ffmpeg_the_third::{
    ChannelLayout,
    ffi::{
        AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, SwrContext, swr_alloc_set_opts2,
        swr_convert_frame, swr_free, swr_init,
    },
    format::Sample,
    frame::Audio,
};
use flume::Sender;
use hound::{WavSpec, WavWriter};
use reqwest::Client;
use tokio::{runtime::Handle, sync::Notify, time::sleep};
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
    audio_frame_bytes_sender: Sender<Vec<u8>>,
}
impl Transcriber {
    pub fn new(args: TranscriberArgs) -> PlayerResult<(Self, Child)> {
        let exe_path = CURRENT_EXE_PATH.as_ref().map_err(anyhow::Error::msg)?;
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
            if let Ok(child_process) = whisper_command.spawn() {
                let mut server_url = String::from("http://127.0.0.1:");
                server_url.push_str(&random_port_str);
                server_url.push_str("/inference");
                break (child_process, server_url);
            }
        };
        let transcribe_task_cancel_token = Arc::new(CancellationToken::new());
        let transcribe_task_notify_cloned = args.transcribe_task_notify.clone();
        let transcribe_task_cancel_token_cloned = transcribe_task_cancel_token.clone();
        let (audio_frame_bytes_sender, audio_frame_bytes_receiver) = flume::bounded(256);
        let target_language = args.target_language.clone();
        let pause_flag = args.pause_flag.clone();
        let subtitle_sender = args.subtitle_sender.clone();
        let _transcribe_task_handle = args.async_runtime.spawn(async move {
            let mut buffer_queue = VecDeque::new();
            let network_client = Client::new();
            while !transcribe_task_cancel_token_cloned.is_cancelled() {
                let used_model = target_language.load();
                if !pause_flag.load(std::sync::atomic::Ordering::Relaxed)
                    && TargetLanguage::None != used_model
                {
                    let frame_bytes = audio_frame_bytes_receiver
                        .drain()
                        .flatten()
                        .collect::<Vec<u8>>();
                    buffer_queue.extend(frame_bytes);

                    if buffer_queue.len() < THREE_SEC_BYTES_LEN && buffer_queue.len() > 32 {
                        let contiguous_slice = buffer_queue.make_contiguous();
                        if Self::transcribe(
                            &network_client,
                            contiguous_slice,
                            &used_model,
                            &subtitle_sender,
                            &server_url,
                        )
                        .with_cancellation_token(&transcribe_task_cancel_token_cloned)
                        .await
                        .is_none()
                        {
                            break;
                        }
                    } else if buffer_queue.len() >= THREE_SEC_BYTES_LEN {
                        let data_bytes = buffer_queue
                            .drain(0..THREE_SEC_BYTES_LEN)
                            .collect::<Vec<u8>>();
                        if Self::transcribe(
                            &network_client,
                            &data_bytes,
                            &used_model,
                            &subtitle_sender,
                            &server_url,
                        )
                        .with_cancellation_token(&transcribe_task_cancel_token_cloned)
                        .await
                        .is_none()
                        {
                            break;
                        }
                    }
                    // have to be above notified, otherwise the clean step will fail
                    sleep(Duration::from_millis(200)).await;
                } else {
                    transcribe_task_notify_cloned.notified().await;
                    info!("transcribe task waked");
                }
            }
        });
        Ok((
            Self {
                audio_resampler: resampler_ctx,
                audio_frame_bytes_sender,
            },
            child_process,
        ))
    }
    async fn transcribe(
        network_client: &Client,
        contiguous_slice: &[u8],
        target_language: &TargetLanguage,
        subtitle_sender: &Sender<String>,
        server_url: &str,
    ) {
        if let Ok(audio_script) = Self::send_request(
            network_client,
            contiguous_slice,
            target_language,
            server_url,
        )
        .await
        {
            for line in audio_script.lines() {
                if let Err(e) = subtitle_sender.send_async(line.to_string()).await {
                    warn!("subtitle_sender err:{:?}", e);
                }
            }
        }
    }
    async fn package_wav_bytes(pcm_data: &[u8]) -> Result<Vec<u8>, hound::Error> {
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
    pub async fn push_audio_frame(&mut self, frame: Audio) -> PlayerResult<()> {
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
        let frame_bytes =
            transcribe_frame.data(0)[0..(transcribe_frame.samples() * size_of::<i16>())].to_vec();
        self.audio_frame_bytes_sender
            .send_async(frame_bytes)
            .await?;

        Ok(())
    }
    async fn send_request(
        network_client: &Client,
        bytes: &[u8],
        target_language: &TargetLanguage,
        server_url: &str,
    ) -> PlayerResult<String> {
        let model_str = match target_language {
            TargetLanguage::None => {
                return Err(anyhow::Error::msg(
                    "used_model should not be none in send_request",
                ));
            }
            TargetLanguage::English => String::from_str("en")?,
            TargetLanguage::Chinese => String::from_str("zh")?,
        };
        let wav_bytes_with_header = Self::package_wav_bytes(bytes).await?;
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
            .post(server_url)
            .multipart(form)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?["text"]
            .as_str()
            .context("parse serde_json::Value to str err!")?
            .to_string();
        Ok(audio_scripts)
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
#[repr(u8)]
#[derive(Debug, PartialEq, Clone)]
pub enum TargetLanguage {
    None = 0,
    Chinese = 1,
    English = 2,
}
#[derive(Debug)]
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
#[derive(Debug, Clone, TypedBuilder)]
pub struct TranscriberArgs {
    async_runtime: Handle,
    subtitle_sender: Sender<String>,
    pause_flag: Arc<AtomicBool>,
    transcribe_task_notify: Arc<Notify>,
    target_language: Arc<AtomicTargetLanguage>,
}
