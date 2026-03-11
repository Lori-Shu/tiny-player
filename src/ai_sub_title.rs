// use std::{fs::File, ptr::null_mut, sync::Arc};

// use anyhow::Error;
// use candle_core::IndexOp;
// use candle_nn::ops::softmax;
// use candle_transformers::{
//     models::{
//         mimi::candle::{Device, Tensor},
//         whisper::{self, Config, quantized_model::Whisper},
//     },
//     quantized_var_builder::VarBuilder,
// };
// use ffmpeg_the_third::{
//     ChannelLayout,
//     ffi::{
//         AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, swr_alloc_set_opts2, swr_convert_frame,
//         swr_free, swr_init,
//     },
//     format::sample::Type,
//     frame::Audio,
// };

// use rustfft::{Fft, FftPlanner, num_complex::Complex};
// use tokenizers::{Tokenizer, tokenizer};
// use tokio::sync::mpsc::Sender;
// use tracing::{info, warn};

// use crate::{CURRENT_EXE_PATH, PlayerResult, decode::ManualProtectedResampler};
// const MEL_FILTERS: &[u8] = include_bytes!("../resources/melfilters.bytes");
// #[derive(PartialEq, Clone)]
// pub enum UsedModel {
//     Empty,
//     Chinese,
//     English,
// }
// struct TokenIds {
//     sot_id: u32,
//     zh_language_id: u32,
//     en_language_id: u32,
//     transcribe_id: u32,
//     no_timestamp_id: u32,
//     eot_id: u32,
//     no_speech_id: u32,
// }
// pub struct AISubTitle {
//     _config: Config,
//     device: Device,
//     model: Whisper,
//     tokenizer: Tokenizer,
//     subtitle_source_resampler: ManualProtectedResampler,
//     subtitle_sender: Sender<String>,
//     candle_streaming_mel_processor: CandleStreamingMelProcessor,
//     token_ids: TokenIds,
// }
// impl AISubTitle {
//     pub fn new(subtitle_sender: Sender<String>) -> PlayerResult<Self> {
//         let current_exe_path = CURRENT_EXE_PATH.as_ref().map_err(Error::msg)?;
//         if let Some(folder_path) = current_exe_path.parent() {
//             let model_path = folder_path.join("model/base_q8_0.gguf");
//             let config_path = folder_path.join("model/config.json");
//             let tokenizer_path = folder_path.join("model/tokenizer.json");
//             if let Some(model_path_str) = model_path.to_str() {
//                 let device = Device::cuda_if_available(0)?;
//                 let var_builder = VarBuilder::from_gguf(model_path_str, &device)?;

//                 let config = serde_json::from_reader::<_, Config>(File::open(config_path)?)?;
//                 let whisper_recognizer =
//                     candle_transformers::models::whisper::quantized_model::Whisper::load(
//                         &var_builder,
//                         config.clone(),
//                     )?;
//                 let tokenizer =
//                     tokenizer::Tokenizer::from_file(tokenizer_path).map_err(anyhow::Error::msg)?;

//                 unsafe {
//                     let mut swr_ctx = null_mut();
//                     let r = swr_alloc_set_opts2(
//                         &mut swr_ctx,
//                         &AV_CHANNEL_LAYOUT_MONO,
//                         ffmpeg_the_third::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
//                         16000,
//                         &AV_CHANNEL_LAYOUT_STEREO,
//                         ffmpeg_the_third::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
//                         48000,
//                         0,
//                         null_mut(),
//                     );
//                     if r < 0 {
//                         info!("swr ctx create err");
//                     }
//                     let r = swr_init(swr_ctx);
//                     if r < 0 {
//                         info!("swr init err");
//                     }
//                     let mel_filters = bytemuck::cast_slice::<_, f32>(MEL_FILTERS).to_vec();
//                     let sot_id = tokenizer
//                         .token_to_id(candle_transformers::models::whisper::SOT_TOKEN)
//                         .ok_or(Error::msg("SOT_TOKEN token2id is none!!!"))?;

//                     let transcribe_id = tokenizer
//                         .token_to_id(candle_transformers::models::whisper::TRANSCRIBE_TOKEN)
//                         .ok_or(anyhow::Error::msg("TRANSCRIBE_TOKEN to id err"))?;

//                     let no_timestamp_id = tokenizer
//                         .token_to_id(candle_transformers::models::whisper::NO_TIMESTAMPS_TOKEN)
//                         .ok_or(anyhow::Error::msg("NO_TIMESTAMPS_TOKEN to id err"))?;
//                     let eot_id = tokenizer
//                         .token_to_id(candle_transformers::models::whisper::EOT_TOKEN)
//                         .ok_or(anyhow::Error::msg("EOT_TOKEN to id err"))?;
//                     let no_speech_token = whisper::NO_SPEECH_TOKENS
//                         .iter()
//                         .find_map(|token| tokenizer.token_to_id(token));
//                     let no_speech_id = match no_speech_token {
//                         None => {
//                             return Err(anyhow::Error::msg("unable to find any non-speech token"));
//                         }
//                         Some(n) => n,
//                     };
//                     let candle_streaming_mel_processor = CandleStreamingMelProcessor::new(
//                         Tensor::new(mel_filters, &device)?,
//                         &device,
//                     )?;
//                     return Ok(Self {
//                         candle_streaming_mel_processor,
//                         subtitle_sender,

//                         _config: config,
//                         device,
//                         tokenizer,
//                         model: whisper_recognizer,
//                         subtitle_source_resampler: ManualProtectedResampler(swr_ctx),
//                         token_ids: TokenIds {
//                             sot_id,
//                             zh_language_id: 50260,
//                             en_language_id: 50259,
//                             transcribe_id,
//                             no_timestamp_id,
//                             eot_id,
//                             no_speech_id,
//                         },
//                     });
//                 }
//             }
//         }

//         Err(anyhow::Error::msg("AISubTitle construct err"))
//     }

//     pub async fn push_frame_data(
//         &mut self,
//         audio_frame: ffmpeg_the_third::frame::Audio,
//         used_model: UsedModel,
//     ) {
//         let mut to_recognize_frame = Audio::empty();
//         to_recognize_frame.set_format(ffmpeg_the_third::format::Sample::F32(Type::Packed));
//         to_recognize_frame.set_ch_layout(ChannelLayout::MONO);
//         to_recognize_frame.set_rate(16000);
//         unsafe {
//             let resampler = &mut self.subtitle_source_resampler;

//             if 0 > swr_convert_frame(
//                 resampler.0,
//                 to_recognize_frame.as_mut_ptr(),
//                 audio_frame.as_ptr(),
//             ) {
//                 warn!("subtitle frame convert err!");
//             }
//         }
//         let data = &bytemuck::cast_slice::<_, f32>(to_recognize_frame.data(0))
//             [0..to_recognize_frame.samples()];
//         if let Ok(Some(t)) = self
//             .candle_streaming_mel_processor
//             .process_chunk(data.to_vec())
//         {
//             if let Ok(audio_features) = self.model.encoder.forward(&t, true) {
//                 if let UsedModel::Chinese = &used_model {
//                     let mut tokens = vec![
//                         self.token_ids.sot_id,
//                         self.token_ids.zh_language_id,
//                         self.token_ids.transcribe_id,
//                         self.token_ids.no_timestamp_id,
//                     ];
//                     let max_tokens = 56;

//                     for i in 0..max_tokens {
//                         if let Ok(input_ids) = Tensor::new(tokens.as_slice(), &self.device) {
//                             if let Ok(input_ids) = input_ids.unsqueeze(0) {
//                                 if let Ok(ys) =
//                                     self.model
//                                         .decoder
//                                         .forward(&input_ids, &audio_features, true)
//                                 {
//                                     if i == 0 {
//                                         if let Ok(t) = ys.i(..1) {
//                                             if let Ok(t) = self.model.decoder.final_linear(&t) {
//                                                 if let Ok(t) = t.i(0) {
//                                                     if let Ok(logits) = t.i(0) {
//                                                         if let Ok(n) = softmax(&logits, 0) {
//                                                             if let Ok(n) = n
//                                                                 .i(self.token_ids.no_speech_id
//                                                                     as usize)
//                                                             {
//                                                                 if let Ok(no_speech_prob) =
//                                                                     n.to_scalar::<f32>()
//                                                                 {
//                                                                     if (no_speech_prob as f64)>candle_transformers::models::whisper::NO_SPEECH_THRESHOLD {
//                                                                                     break;
//                                                                                 }
//                                                                 }
//                                                             }
//                                                         }
//                                                     }
//                                                 }
//                                             }
//                                         }
//                                     }
//                                     if let Ok((_, seq_len, _)) = ys.dims3() {
//                                         if let Ok(t) = ys.i((..1, seq_len - 1..)) {
//                                             if let Ok(t) = self.model.decoder.final_linear(&t) {
//                                                 if let Ok(t) = t.i(0) {
//                                                     if let Ok(logits) = t.i(0) {
//                                                         let logits_v: candle_core::Result<
//                                                             Vec<f32>,
//                                                         > = logits.to_vec1();
//                                                         if let Ok(logits_v) = logits_v {
//                                                             if let Some(v) = logits_v
//                                                                 .iter()
//                                                                 .enumerate()
//                                                                 .max_by(|(_, u), (_, v)| {
//                                                                     u.total_cmp(v)
//                                                                 })
//                                                                 .map(|(i, _)| i as u32)
//                                                             {
//                                                                 let next_token = v;
//                                                                 tokens.push(next_token);
//                                                                 if next_token
//                                                                     == self.token_ids.eot_id
//                                                                     || tokens.len() > max_tokens
//                                                                 {
//                                                                     break;
//                                                                 }
//                                                             }
//                                                         }
//                                                     }
//                                                 }
//                                             }
//                                         }
//                                     }
//                                 }
//                             }
//                         }
//                     }
//                     if let Ok(text) = self.tokenizer.decode(&tokens, true) {
//                         if let Err(e) = self.subtitle_sender.send(text.trim().to_string()).await {
//                             warn!("send subtitle err {}", e.to_string());
//                         }
//                     }
//                 } else if let UsedModel::English = &used_model {
//                     let mut tokens = vec![
//                         self.token_ids.sot_id,
//                         self.token_ids.en_language_id,
//                         self.token_ids.transcribe_id,
//                         self.token_ids.no_timestamp_id,
//                     ];
//                     let max_tokens = 56;

//                     for i in 0..max_tokens {
//                         if let Ok(input_ids) = Tensor::new(tokens.as_slice(), &self.device) {
//                             if let Ok(input_ids) = input_ids.unsqueeze(0) {
//                                 if let Ok(ys) =
//                                     self.model
//                                         .decoder
//                                         .forward(&input_ids, &audio_features, true)
//                                 {
//                                     if i == 0 {
//                                         if let Ok(t) = ys.i(..1) {
//                                             if let Ok(t) = self.model.decoder.final_linear(&t) {
//                                                 if let Ok(t) = t.i(0) {
//                                                     if let Ok(logits) = t.i(0) {
//                                                         if let Ok(n) = softmax(&logits, 0) {
//                                                             if let Ok(n) = n
//                                                                 .i(self.token_ids.no_speech_id
//                                                                     as usize)
//                                                             {
//                                                                 if let Ok(no_speech_prob) =
//                                                                     n.to_scalar::<f32>()
//                                                                 {
//                                                                     if (no_speech_prob as f64) > 0.8
//                                                                     {
//                                                                         break;
//                                                                     }
//                                                                 }
//                                                             }
//                                                         }
//                                                     }
//                                                 }
//                                             }
//                                         }
//                                     }

//                                     if let Ok((_, seq_len, _)) = ys.dims3() {
//                                         // 3. ✨ 关键：切出最后一个位置的隐藏状态 [1, 1, hidden_dim]
//                                         // 这样做是为了效率，因为我们只需要预测下一个 token
//                                         if let Ok(t) = ys.i((..1, seq_len - 1..)) {
//                                             if let Ok(t) = self.model.decoder.final_linear(&t) {
//                                                 if let Ok(t) = t.i(0) {
//                                                     if let Ok(logits) = t.i(0) {
//                                                         let logits_v: candle_core::Result<
//                                                             Vec<f32>,
//                                                         > = logits.to_vec1();
//                                                         if let Ok(logits_v) = logits_v {
//                                                             if let Some(v) = logits_v
//                                                                 .iter()
//                                                                 .enumerate()
//                                                                 .max_by(|(_, u), (_, v)| {
//                                                                     u.total_cmp(v)
//                                                                 })
//                                                                 .map(|(i, _)| i as u32)
//                                                             {
//                                                                 let next_token = v;
//                                                                 tokens.push(next_token);
//                                                                 if next_token
//                                                                     == self.token_ids.eot_id
//                                                                     || tokens.len() > max_tokens
//                                                                 {
//                                                                     break;
//                                                                 }
//                                                             }
//                                                         }
//                                                     }
//                                                 }
//                                             }
//                                         }
//                                     }
//                                 }
//                             }
//                         }
//                     }
//                     if let Ok(text) = self.tokenizer.decode(&tokens, true) {
//                         warn!("after decode text==={}", text);
//                         if let Err(e) = self.subtitle_sender.send(text).await {
//                             warn!("send subtitle err {}", e.to_string());
//                         }
//                     }
//                 }
//             }
//         }
//     }
// }
// impl Drop for AISubTitle {
//     fn drop(&mut self) {
//         unsafe {
//             swr_free(&mut self.subtitle_source_resampler.0);
//         }
//     }
// }

// pub struct CandleStreamingMelProcessor {
//     device: Device,
//     mel_filters: Tensor, // (80, 201)
//     hann_window: Vec<f32>,
//     audio_buffer: Vec<f32>,
//     fft_handler: Arc<dyn Fft<f32>>,
//     n_fft: usize,
//     hop_length: usize,
//     mel_buffer: Vec<Tensor>, // 存储单帧 Tensor
//     max_mel_frames: usize,
// }

// impl CandleStreamingMelProcessor {
//     pub fn new(mel_filters: Tensor, device: &Device) -> PlayerResult<Self> {
//         let n_fft = 400;
//         let hann: Vec<f32> = (0..n_fft)
//             .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n_fft as f32).cos())
//             .collect();

//         let mut planner = FftPlanner::new();
//         let fft_handler = planner.plan_fft_forward(n_fft);

//         // 确保 mel_filters 形状正确并搬运到目标设备
//         let mel_filters = mel_filters.reshape((80, 201))?.to_device(device)?;

//         Ok(Self {
//             device: device.clone(),
//             mel_filters,
//             hann_window: hann,
//             audio_buffer: Vec::new(),
//             fft_handler,
//             n_fft,
//             hop_length: 160,
//             mel_buffer: vec![],
//             max_mel_frames: 3000,
//         })
//     }

//     /// 将音频切片转换为功率谱 Tensor (Batch 处理以提升效率)
//     fn frames_to_power_spec(&self, frames: &[&[f32]]) -> candle_core::Result<Tensor> {
//         let n_out = self.n_fft / 2 + 1;
//         let mut all_energies = Vec::with_capacity(frames.len() * n_out);

//         for frame in frames {
//             let mut buffer: Vec<Complex<f32>> = frame
//                 .iter()
//                 .zip(self.hann_window.iter())
//                 .map(|(&x, &w)| Complex::new(x * w, 0.0))
//                 .collect();

//             self.fft_handler.process(&mut buffer);

//             // Power Spectrum: |FFT|^2
//             for c in &buffer[..n_out] {
//                 all_energies.push(c.norm_sqr());
//             }
//         }

//         Tensor::from_vec(all_energies, (frames.len(), n_out), &self.device)
//     }

//     // 修正后的核心处理逻辑
//     pub fn process_chunk(&mut self, pcm: Vec<f32>) -> PlayerResult<Option<Tensor>> {
//         self.audio_buffer.extend(pcm);

//         // 至少需要一个 FFT 窗口的数据
//         if self.audio_buffer.len() < self.n_fft {
//             return Ok(None);
//         }

//         // 1. 提取所有可用的帧
//         let num_frames = (self.audio_buffer.len() - self.n_fft) / self.hop_length + 1;
//         let mut frames = Vec::with_capacity(num_frames);
//         for i in 0..num_frames {
//             let start = i * self.hop_length;
//             frames.push(&self.audio_buffer[start..start + self.n_fft]);
//         }

//         // 2. 批量计算功率谱
//         let power_spec = self.frames_to_power_spec(&frames)?;

//         // 3. 矩阵乘法转 Mel: [frames, 201] * [201, 80] -> [frames, 80]
//         let mel = power_spec.matmul(&self.mel_filters.t()?)?;

//         // 4. Log-Mel 变换与归一化 (核心修正点)
//         // Whisper 标准: log10(mel) -> 映射到 [-1, 1]
//         let shape = mel.shape().clone();

//         // 1. 将 Tensor 转回 Rust 的 Vec<f32> 进行高精度 log10 处理
//         let mel_vec = mel.flatten_all()?.to_vec1::<f32>()?;

//         // 2. 使用 Rust 标准库 f32::log10 处理
//         // 我们同时在这里处理 clamp，防止 log10(-inf)
//         let log_mel_vec: Vec<f32> = mel_vec.into_iter().map(|v| v.max(1e-10).log10()).collect();

//         // 3. 转回 Tensor
//         let log10_mel = Tensor::from_vec(log_mel_vec, shape, &self.device)?;

//         // 1. 获取当前批次的最大值
//         let max_val = log10_mel.max_all()?.to_scalar::<f32>()?;

//         // 2. 创建一个标量 Tensor 作为阈值 (注意必须在同一个 device 上)
//         let threshold = Tensor::from_vec(vec![max_val - 8.0], (1, 1), &self.device)?
//             .broadcast_as(log10_mel.shape())?;

//         // 3. 使用 broadcast_maximum (或简写 maximum)
//         let log_mel = log10_mel.maximum(&threshold)?.affine(1.0 / 4.0, 1.0)?;

//         // 5. 逐帧推入缓冲区
//         // 修正：从 log_mel 中提取每一帧时，保持 Tensor 引用
//         for i in 0..num_frames {
//             let frame = log_mel.narrow(0, i, 1)?.squeeze(0)?;
//             self.mel_buffer.push(frame);
//         }

//         // 清理 PCM 缓冲区
//         self.audio_buffer.drain(0..num_frames * self.hop_length);

//         // 6. 限制缓冲区并返回
//         if self.mel_buffer.len() > self.max_mel_frames {
//             self.mel_buffer
//                 .drain(0..self.mel_buffer.len() - self.max_mel_frames);
//         }

//         // 如果帧数太少（比如少于 0.5s），Encoder 可能无法捕捉特征
//         if self.mel_buffer.len() < 200 {
//             return Ok(None);
//         }

//         // 重点：Whisper Encoder 期望的形状是 [Batch, 80, Time]
//         let final_mel = Tensor::stack(&self.mel_buffer, 0)
//             ?
//             .t() // [80, Time]
//             ?
//             .unsqueeze(0) // [1, 80, Time]
//             ?;
//         self.mel_buffer.clear();
//         let (_, _, current_frames) = final_mel.dims3()?;
//         let final_mel = if current_frames < 3000 {
//             let pad_size = 3000 - current_frames;
//             let pad = Tensor::zeros((1, 80, pad_size), final_mel.dtype(), final_mel.device())
//                 ?
//                 .affine(1.0, -1.0) // 0.0 * 1.0 + (-1.0) = -1.0
//                 ?;

//             Tensor::cat(&[final_mel, pad], 2)?
//         } else {
//             final_mel.narrow(2, 0, 3000)?
//         };
//         Ok(Some(final_mel))
//     }
// }
