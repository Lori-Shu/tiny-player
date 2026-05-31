use std::{
    collections::VecDeque,
    ptr::null_mut,
    str::FromStr,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::Context;
use ffmpeg_the_third::{
    ChannelLayout,
    ffi::{
        AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, swr_alloc_set_opts2, swr_convert_frame,
        swr_free, swr_init,
    },
    format::Sample,
    frame::Audio,
};
use flume::Sender;
use reqwest::Client;
use tokio::{
    process::{Child, Command},
    runtime::Handle,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;
use tracing::warn;
use typed_builder::TypedBuilder;

use crate::{
    CURRENT_EXE_PATH, PlayerResult, decode::ManualProtectedResampler,
    present_data_manage::PLAY_SAMPLE_RATE,
};

const TRANSCRIBE_SAMPLE_RATE: u32 = 16000;
const LOCAL_WHISPER_SERVER_URL: &str = "http://127.0.0.1:8187/inference";
#[derive(Debug)]
pub struct Transcriber {
    async_runtime: Handle,
    whisper_command: Option<Child>,
    audio_resampler: ManualProtectedResampler,
    audio_frame_vec_sender: Sender<Vec<u8>>,
    transcribe_task_notify: Arc<Notify>,
    transcribe_task_cancel_token: Arc<CancellationToken>,
    transcribe_task_handle: Option<JoinHandle<()>>,
}
impl Transcriber {
    pub fn new(args: TranscriberArgs) -> PlayerResult<Self> {
        let exe_path = CURRENT_EXE_PATH.as_ref().map_err(anyhow::Error::msg)?;
        let exe_dir = exe_path.parent().context("get parent_dir err")?;
        let model_path = exe_dir.join("models");
        let path_str = model_path.to_str().context("to str failed")?;
        let mut path_str = path_str.to_string();
        path_str.push('/');
        unsafe {
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
            let whisper_command = Some(
                Command::new("whisper-server.exe")
                    .arg("--language")
                    .arg("auto")
                    .arg("--model")
                    .arg("models/ggml-base-q8_0.bin")
                    .arg("--port")
                    .arg("8187")
                    .spawn()?,
            );
            let network_client = Client::new();
            let transcribe_task_cancel_token = Arc::new(CancellationToken::new());
            let transcribe_task_notify_cloned = args.transcribe_task_notify.clone();
            let transcribe_task_cancel_token_cloned = transcribe_task_cancel_token.clone();
            let (audio_frame_vec_sender, audio_frame_vec_receiver) = flume::bounded(100);
            let used_model = args.used_model.clone();
            let pause_flag = args.pause_flag.clone();
            let subtitle_sender = args.subtitle_sender.clone();
            let transcribe_task_handle = Some(args.async_runtime.spawn(async move {
                let mut buffer_queue = VecDeque::new();
                while !transcribe_task_cancel_token_cloned.is_cancelled() {
                    let used_model =  (*used_model.read().await).clone();
                    if !pause_flag.load(std::sync::atomic::Ordering::Relaxed)
                        && UsedModel::None != used_model
                    {
                        let data_vec = audio_frame_vec_receiver
                            .drain()
                            .flatten()
                            .collect::<Vec<u8>>();
                        buffer_queue.extend(data_vec);
                        const THREE_SEC_BYTES_LEN: usize =
                            (TRANSCRIBE_SAMPLE_RATE as usize) * 3 * size_of::<i16>();
                        if buffer_queue.len() < THREE_SEC_BYTES_LEN {
                            let contiguous_slice = buffer_queue.make_contiguous();
                            if let Ok(audio_script) =
                                Self::send_request(&network_client, contiguous_slice, &used_model)
                                    .await
                            {
                                if let Err(e) = subtitle_sender.send_async(audio_script).await {
                                    warn!("subtitle_sender err:{:?}", e);
                                }
                            }
                        } else {
                            let data_bytes = buffer_queue
                                .drain(0..THREE_SEC_BYTES_LEN)
                                .collect::<Vec<u8>>();
                            if let Ok(audio_script) =
                                Self::send_request(&network_client, &data_bytes, &used_model).await
                            {
                                if let Err(e) = subtitle_sender.send_async(audio_script).await {
                                    warn!("subtitle_sender err:{:?}", e);
                                }
                            }
                        }
                    } else {
                        transcribe_task_notify_cloned.notified().await;
                    }
                    sleep(Duration::from_millis(500)).await;
                }
            }));
            Ok(Self {
                transcribe_task_notify: args.transcribe_task_notify,
                transcribe_task_cancel_token,
                async_runtime: args.async_runtime,
                whisper_command,
                audio_resampler: ManualProtectedResampler(swr_ctx),
                transcribe_task_handle,
                audio_frame_vec_sender,
            })
        }
    }
    async fn package_wav_bytes(pcm_data: Vec<u8>) -> Vec<u8> {
        let mut wav = Vec::with_capacity(44 + pcm_data.len());
        let data_len = pcm_data.len() as u32;
        let file_len = data_len + 36;

        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_len.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&TRANSCRIBE_SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(&(TRANSCRIBE_SAMPLE_RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&pcm_data);
        wav
    }
    pub async fn push_audio_frame(&mut self, frame: Audio) -> PlayerResult<()> {
        unsafe {
            let mut to_recognize_frame = Audio::empty();
            to_recognize_frame
                .set_format(Sample::I16(ffmpeg_the_third::format::sample::Type::Packed));
            to_recognize_frame.set_ch_layout(ChannelLayout::MONO);
            to_recognize_frame.set_rate(TRANSCRIBE_SAMPLE_RATE);

            let err_num = swr_convert_frame(
                self.audio_resampler.0,
                to_recognize_frame.as_mut_ptr(),
                frame.as_ptr(),
            );
            if err_num < 0 {
                let err_msg = format!("audio frame convert err: {}", err_num);
                warn!(err_msg);
                return Err(anyhow::Error::msg(err_msg));
            }
            let data_vec = (&to_recognize_frame.data(0)
                [0..(to_recognize_frame.samples() * size_of::<i16>())])
                .to_vec();
            self.audio_frame_vec_sender.send_async(data_vec).await?;
        }
        Ok(())
    }
    async fn send_request(
        network_client: &Client,
        bytes: &[u8],
        used_model: &UsedModel,
    ) -> PlayerResult<String> {
        let model_str = match used_model {
            UsedModel::None => {
                return Ok(String::new());
            }
            UsedModel::English => String::from_str("en")?,
            UsedModel::Chinese => String::from_str("zh")?,
        };
        let wav_bytes_with_header = Self::package_wav_bytes(bytes.to_vec()).await;
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
            .post(LOCAL_WHISPER_SERVER_URL)
            .multipart(form)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?
            .to_string();
        Ok(audio_scripts)
    }
}
impl Drop for Transcriber {
    fn drop(&mut self) {
        unsafe {
            swr_free(&mut self.audio_resampler.0);

            self.transcribe_task_cancel_token.cancel();
            self.transcribe_task_notify.notify_waiters();
            if let Some(transcribe_task_handle) = self.transcribe_task_handle.take()
                && let Some(mut whisper_command) = self.whisper_command.take()
            {
                self.async_runtime.spawn(async move {
                    if let Err(e) = whisper_command.kill().await {
                        warn!("exit err:{:?}", e);
                    }
                    transcribe_task_handle.await?;
                    PlayerResult::Ok(())
                });
            }
        }
    }
}
#[derive(Debug, PartialEq, Clone)]
pub enum UsedModel {
    None,
    Chinese,
    English,
}
#[derive(Debug, Clone, TypedBuilder)]
pub struct TranscriberArgs {
    async_runtime: Handle,
    subtitle_sender: Sender<String>,
    pause_flag: Arc<AtomicBool>,
    transcribe_task_notify: Arc<Notify>,
    used_model: Arc<RwLock<UsedModel>>,
}
