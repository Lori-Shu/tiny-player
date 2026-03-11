use anyhow::Error;
use burn::{
    Tensor,
    backend::{Cuda, cuda::CudaDevice},
    config::Config,
    module::Module,
    record::{FullPrecisionSettings, NamedMpkFileRecorder, Recorder},
    tensor::backend::Backend,
};
use ffmpeg_the_third::{
    ChannelLayout,
    ffi::{
        AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, swr_alloc_set_opts2, swr_convert_frame,
        swr_init,
    },
    frame::Audio,
};
use rustfft::Fft;
use tokio::sync::mpsc::{Sender, UnboundedReceiver};
use tracing::{info, warn};

use crate::{
    decode::ManualProtectedResampler,
    whisper_burn::{
        audio::{N_FFT, WINDOW_LENGTH, get_mel_filters_device, hann_window_device},
        model::{Whisper, WhisperConfig},
        token::{Gpt2Tokenizer, Language, SpecialToken},
        transcribe::waveform_to_text,
    },
};
use std::{
    collections::VecDeque,
    ptr::null_mut,
    sync::{Arc, Condvar},
};
use webrtc_vad::{Vad, VadMode};

use crate::PlayerResult;

const BUFFER_FRAME_COUNT: usize = 35;
const _MINIMUM_SAMPLE_COUNT: usize = 1600 * 10; // @ 16kHz = 400ms
const MAXIMUM_SAMPLE_COUNT: usize = 1600 * 20;

#[derive(PartialEq, Clone)]
pub enum UsedModel {
    Empty,
    Chinese,
    English,
}
pub struct AISubTitle2 {
    speaking: bool,
    unactive_count: usize,
    vad_buffer: VecDeque<i16>,
    speech_segment: VecDeque<f32>,
    subtitle_resampler: ManualProtectedResampler,
    bpe: Gpt2Tokenizer,
    whisper: Whisper<Cuda>,
    _whisper_config: WhisperConfig,
    text_sender: Sender<String>,
    frame_receiver: UnboundedReceiver<(Audio, UsedModel)>,
    condition: Arc<Condvar>,
    fft: Arc<dyn Fft<f32>>,
    mel_filters: Tensor<Cuda, 2>,
    english_initial_tokens: Vec<usize>,
    chinese_initial_tokens: Vec<usize>,
    end_token: usize,
    hann_window: Tensor<Cuda, 1>,
}
impl AISubTitle2 {
    pub fn new(
        text_sender: Sender<String>,
        frame_receiver: UnboundedReceiver<(Audio, UsedModel)>,
        condition: Arc<Condvar>,
    ) -> PlayerResult<Self> {
        let tensor_device = CudaDevice::default();
        let (bpe, whisper_config, whisper) = Self::load_model::<Cuda>(&tensor_device)?;
        info!("Model loaded successfully");
        unsafe {
            let mut swr_ctx = null_mut();
            let r = swr_alloc_set_opts2(
                &mut swr_ctx,
                &AV_CHANNEL_LAYOUT_MONO,
                ffmpeg_the_third::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                16000,
                &AV_CHANNEL_LAYOUT_STEREO,
                ffmpeg_the_third::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                48000,
                0,
                null_mut(),
            );
            if r < 0 {
                info!("swr ctx create err");
            }
            let r = swr_init(swr_ctx);
            if r < 0 {
                info!("swr init err");
            }
            let mut fft_planner = rustfft::FftPlanner::<f32>::new();
            let fft = fft_planner.plan_fft_forward(N_FFT);
            let mel_filters = get_mel_filters_device::<Cuda>(
                16000.0,
                N_FFT,
                whisper.encoder_mel_size(),
                false,
                &tensor_device,
            );
            let start_token = bpe
                .special_token(SpecialToken::StartofTranscript)
                .ok_or(Error::msg("special token err"))?;
            let transcription_token = bpe
                .special_token(SpecialToken::Transcribe)
                .ok_or(Error::msg("special token err"))?;
            let _start_of_prev_token = bpe
                .special_token(SpecialToken::StartofPrev)
                .ok_or(Error::msg("special token err"))?;
            let eng_lang_token = bpe
                .special_token(SpecialToken::Language(Language::English))
                .ok_or(Error::msg("special token err"))?;
            let chinese_lang_token = bpe
                .special_token(SpecialToken::Language(Language::Chinese))
                .ok_or(Error::msg("special token err"))?;
            let _first_timestamp_token = bpe
                .special_token(SpecialToken::Timestamp(0.0))
                .ok_or(Error::msg("special token err"))?;
            let end_token = bpe
                .special_token(SpecialToken::EndofText)
                .ok_or(Error::msg("special token err"))?;
            let notimestamp = bpe
                .special_token(SpecialToken::NoTimeStamps)
                .ok_or(Error::msg("special token err"))?;
            let english_initial_tokens = vec![
                start_token,
                eng_lang_token,
                transcription_token,
                notimestamp,
            ];
            let chinese_initial_tokens = vec![
                start_token,
                chinese_lang_token,
                transcription_token,
                notimestamp,
            ];
            let hann_window = hann_window_device(WINDOW_LENGTH, &tensor_device);
            Ok(Self {
                speaking: false,
                unactive_count: 0,
                vad_buffer: VecDeque::new(),
                speech_segment: VecDeque::new(),
                subtitle_resampler: ManualProtectedResampler(swr_ctx),
                bpe,
                whisper,
                _whisper_config: whisper_config,
                text_sender,
                frame_receiver,
                condition,
                fft,
                mel_filters,
                english_initial_tokens,
                chinese_initial_tokens,
                hann_window,
                end_token,
            })
        }
    }
    pub fn run_transcribe_loop(&mut self) {
        let condvar_mutex = std::sync::Mutex::new(());
        loop {
            if let Err(e) = self.receive_frame_data() {
                warn!("run_transcribe_loop err{}", e);
            }
            if let Ok(guard) = condvar_mutex.lock() {
                if let Err(e) = self.condition.wait(guard) {
                    warn!("wait on cond var err{}", e);
                }
            }
        }
    }
    fn receive_frame_data(&mut self) -> PlayerResult<()> {
        if let Ok((audio_frame, used_model)) = self.frame_receiver.try_recv() {
            let mut vad = Vad::new_with_rate(webrtc_vad::SampleRate::Rate16kHz);
            vad.set_mode(VadMode::Aggressive);
            let mut to_recognize_frame = Audio::empty();
            to_recognize_frame.set_format(ffmpeg_the_third::format::Sample::F32(
                ffmpeg_the_third::format::sample::Type::Packed,
            ));
            to_recognize_frame.set_ch_layout(ChannelLayout::MONO);
            to_recognize_frame.set_rate(16000);
            unsafe {
                if 0 > swr_convert_frame(
                    self.subtitle_resampler.0,
                    to_recognize_frame.as_mut_ptr(),
                    audio_frame.as_ptr(),
                ) {
                    warn!("subtitle frame convert err!");
                }
            }
            let frame_slice = &to_recognize_frame.data(0)[0..(to_recognize_frame.samples() * 4)];
            let data = bytemuck::cast_slice::<_, f32>(frame_slice).to_vec();
            let data = data
                .into_iter()
                .map(|n| (n * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
                .collect::<Vec<i16>>();
            self.vad_buffer.extend(data);
            if self.vad_buffer.len() > 160 {
                let vad_buf = self.vad_buffer.drain(0..160).collect::<Vec<i16>>();
                let speech_active = vad
                    .is_voice_segment(&vad_buf)
                    .map_err(|_| Error::msg("is_voice_segment err"))?;
                if self.speaking {
                    if speech_active {
                        self.speech_segment
                            .extend(bytemuck::cast_slice(to_recognize_frame.data(0)));
                        if self.speech_segment.len() > MAXIMUM_SAMPLE_COUNT {
                            self.process_audio_data(used_model)?;
                        }
                    } else {
                        if self.unactive_count > BUFFER_FRAME_COUNT {
                            /*
                                If more than 30 frames of unactive speech
                                then consider end of segment and
                                send over the channel to transcribing service
                            */

                            if self.speech_segment.len() > MAXIMUM_SAMPLE_COUNT {
                                self.process_audio_data(used_model)?;
                            }
                            self.speaking = false;
                        } else {
                            self.unactive_count += 1;
                        }
                    }
                } else {
                    if speech_active {
                        self.speaking = true;
                        self.unactive_count = 0;
                        self.speech_segment
                            .extend(bytemuck::cast_slice(to_recognize_frame.data(0)));
                    }
                }
            }
        }
        Ok(())
    }
    fn load_model<B: Backend>(
        tensor_device_ref: &B::Device,
    ) -> PlayerResult<(Gpt2Tokenizer, WhisperConfig, Whisper<B>)> {
        let bpe = match Gpt2Tokenizer::new("./resources/whisper_tiny/tokenizer.json") {
            Ok(bpe) => bpe,
            Err(e) => {
                return Err(anyhow::Error::msg(e));
            }
        };

        let whisper_config = match WhisperConfig::load("./resources/whisper_tiny/tiny.cfg") {
            Ok(config) => config,
            Err(e) => {
                return Err(Error::msg(format!("load_whisper err{}", e)));
            }
        };

        info!("Loading model...");
        let whisper: Whisper<B> = {
            match NamedMpkFileRecorder::<FullPrecisionSettings>::new()
                .load("./resources/whisper_tiny/tiny".into(), tensor_device_ref)
                .map(|record| whisper_config.init(tensor_device_ref).load_record(record))
            {
                Ok(whisper_model) => whisper_model,
                Err(e) => {
                    return Err(Error::from(e));
                }
            }
        };

        let whisper = whisper.to_device(&tensor_device_ref);

        Ok((bpe, whisper_config, whisper))
    }
    fn process_audio_data(&mut self, used_model: UsedModel) -> PlayerResult<()> {
        // //LEAVING THIS FOR NOW AS THEIR IS STILL A BIT OF AUDIO DISTORTION AND WANT TO DEBUG LATER
        // let spec = hound::WavSpec {
        //     channels: 1,
        //     sample_rate: 16000, // adjust this to match your audio data
        //     bits_per_sample: 16, // adjust this to match your audio data
        //     sample_format: hound::SampleFormat::Int,
        // };

        // let mut writer = hound::WavWriter::create(format!("output_{}.wav", i), spec).unwrap();
        // let mut audio_data_vectors_clone_for_inference2: Vec<i16> = audio_data_vectors.clone().into();
        // for sample in audio_data_vectors_clone_for_inference2 {
        //     writer.write_sample(sample).unwrap(); // cast to i16, adjust this to match your audio data
        // }

        // writer.finalize().unwrap();
        info!("before INFERENCE");
        let initial_tokens = match used_model {
            UsedModel::English => &self.english_initial_tokens,
            UsedModel::Chinese => &self.chinese_initial_tokens,
            _ => &self.english_initial_tokens,
        };
        //RUN INFERENCE
        let (text, _tokens) = match waveform_to_text(
            &self.whisper,
            &self.bpe,
            self.speech_segment
                .drain(0..MAXIMUM_SAMPLE_COUNT)
                .collect::<Vec<f32>>(),
            16000,
            true,
            self.fft.clone(),
            &self.mel_filters,
            &self.hann_window,
            initial_tokens,
            &self.end_token,
        ) {
            Ok((text, tokens)) => (text, tokens),
            Err(e) => {
                return Err(Error::msg(e));
            }
        };
        // info!("recognized text{}", text);
        self.text_sender.try_send(text)?;
        // info!("process_audio_data end");
        // info!("\nText: {}, Iteration: {}, Time:{:?}", text, i, start_time.elapsed());
        Ok(())
    }
}
