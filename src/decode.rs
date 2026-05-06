use std::{
    path::Path,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
    time::Duration,
};

use anyhow::Context;
use derive_builder::Builder;
use ffmpeg_the_third::{
    ChannelLayout, Packet, Rational, Stream, codec,
    ffi::{
        AV_CHANNEL_LAYOUT_STEREO, AV_NOPTS_VALUE, AVCodecContext, AVHWDeviceType, AVPixelFormat,
        AVSEEK_FLAG_BACKWARD, AVSampleFormat, SwrContext, av_hwdevice_ctx_create,
        av_hwframe_transfer_data, avcodec_get_hw_config, swr_alloc_set_opts2, swr_convert_frame,
        swr_free, swr_init,
    },
    format::{Sample, sample::Type, stream::Disposition},
    frame::{Audio, Video},
};

use flume::{Receiver, Sender};
use time::format_description;
use tokio::{
    runtime::Handle,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};
use tracing::{Instrument, Level, info, span, warn};

use crate::{PlayerResult, audio_play::AUDIO_SAMPLE_RATE, gpu_post_process::ColorSpaceConverter};
/// this wrapper type should be protected manually to
/// keep memory safe in multi threads
/// means need to wrap an Arc and a Lock to use it in multi threads
pub struct ManualProtectedInput(ffmpeg_the_third::format::context::Input);
unsafe impl Send for ManualProtectedInput {}
unsafe impl Sync for ManualProtectedInput {}
/// this wrapper type should be protected manually to
/// keep memory safe in multi threads
/// means need to wrap an Arc and a Lock to use it in multi threads
pub struct ManualProtectedVideoDecoder(ffmpeg_the_third::decoder::Video);

unsafe impl Sync for ManualProtectedVideoDecoder {}
/// this wrapper type should be protected manually to
/// keep memory safe in multi threads
/// means need to wrap an Arc and a Lock to use it in multi threads
pub struct ManualProtectedAudioDecoder(ffmpeg_the_third::decoder::Audio);

unsafe impl Sync for ManualProtectedAudioDecoder {}
/// this wrapper type should be protected manually to
/// keep memory safe in multi threads
/// means need to wrap an Arc and a Lock to use it in multi threads
pub struct ManualProtectedResampler(pub *mut SwrContext);
unsafe impl Send for ManualProtectedResampler {}
unsafe impl Sync for ManualProtectedResampler {}

struct ManualProtectedStream<'stream_life_time>(pub Stream<'stream_life_time>);
unsafe impl<'stream_life_time> Send for ManualProtectedStream<'stream_life_time> {}
unsafe impl<'stream_life_time> Sync for ManualProtectedStream<'stream_life_time> {}

/// indicate which stream in the input is chose as main stream
#[derive(Debug, Clone)]
pub enum MainStream {
    Video,
    Audio,
}

/// represent all the details and relevent variables about
/// video format, decode, detail and hardware accelerate
/// the main struct of decode module to manage input and decode
pub struct TinyDecoder {
    video_stream_index: usize,
    audio_stream_index: usize,
    cover_stream_index: usize,
    pub main_stream: MainStream,
    pub video_time_base: Rational,
    pub audio_time_base: Rational,
    pub video_frame_rect: [u32; 2],
    format_duration: i64,
    end_timestamp: Arc<AtomicI64>,
    pub end_time_formatted_string: String,
    format_input: Arc<RwLock<Option<ManualProtectedInput>>>,
    video_decoder: Arc<RwLock<Option<ManualProtectedVideoDecoder>>>,
    audio_decoder: Arc<RwLock<Option<ManualProtectedAudioDecoder>>>,
    resampler: Arc<RwLock<Option<ManualProtectedResampler>>>,
    video_frame_cache_queue: (
        Sender<ffmpeg_the_third::frame::Video>,
        Receiver<ffmpeg_the_third::frame::Video>,
    ),
    audio_frame_cache_queue: (
        Sender<ffmpeg_the_third::frame::Audio>,
        Receiver<ffmpeg_the_third::frame::Audio>,
    ),
    audio_packet_cache_queue: (
        Sender<ffmpeg_the_third::packet::Packet>,
        Receiver<ffmpeg_the_third::packet::Packet>,
    ),
    video_packet_cache_queue: (
        Sender<ffmpeg_the_third::packet::Packet>,
        Receiver<ffmpeg_the_third::packet::Packet>,
    ),
    demux_exit_flag: Arc<AtomicBool>,
    decode_exit_flag: Arc<AtomicBool>,
    demux_task_handle: Option<JoinHandle<PlayerResult<()>>>,
    video_decode_task_handle: Option<JoinHandle<PlayerResult<()>>>,
    audio_decode_task_handle: Option<JoinHandle<PlayerResult<()>>>,
    hardware_config_flag: Arc<AtomicBool>,
    pub cover_pic_data: Arc<RwLock<Option<Vec<u8>>>>,
    runtime_handle: Handle,
    demux_thread_notify: Arc<Notify>,
    audio_decode_thread_notify: Arc<Notify>,
    video_decode_thread_notify: Arc<Notify>,
    color_space_converter: Arc<RwLock<ColorSpaceConverter>>,
    media_source_flag: Arc<AtomicBool>,
}
impl TinyDecoder {
    /// init Decoder and new Struct
    /// `runtime_handle` is the handle of the tokio runtime in async_context
    pub fn new(
        runtime_handle: Handle,
        media_source_flag: Arc<AtomicBool>,
        end_timestamp: Arc<AtomicI64>,
        hardware_config_flag: Arc<AtomicBool>,
        color_space_converter: Arc<RwLock<ColorSpaceConverter>>,
        audio_frame_cache_queue: (
            Sender<ffmpeg_the_third::frame::Audio>,
            Receiver<ffmpeg_the_third::frame::Audio>,
        ),
        video_frame_cache_queue: (
            Sender<ffmpeg_the_third::frame::Video>,
            Receiver<ffmpeg_the_third::frame::Video>,
        ),
        audio_decode_thread_notify: Arc<Notify>,
        video_decode_thread_notify: Arc<Notify>,
    ) -> PlayerResult<Self> {
        ffmpeg_the_third::init()?;
        let resampler = Arc::new(RwLock::new(None));
        Ok(Self {
            video_stream_index: usize::MAX,
            audio_stream_index: usize::MAX,
            cover_stream_index: usize::MAX,
            main_stream: MainStream::Audio,
            video_time_base: Rational::new(1, 1),
            audio_time_base: Rational::new(1, 1),
            video_frame_rect: [0, 0],
            format_duration: 0,
            end_timestamp,
            end_time_formatted_string: String::new(),
            format_input: Arc::new(RwLock::new(None)),
            video_decoder: Arc::new(RwLock::new(None)),
            audio_decoder: Arc::new(RwLock::new(None)),
            video_frame_cache_queue,
            audio_frame_cache_queue,
            audio_packet_cache_queue: flume::bounded(512),
            video_packet_cache_queue: flume::bounded(512),
            demux_exit_flag: Arc::new(AtomicBool::new(false)),
            decode_exit_flag: Arc::new(AtomicBool::new(false)),
            demux_task_handle: None,
            video_decode_task_handle: None,
            audio_decode_task_handle: None,
            hardware_config_flag,
            cover_pic_data: Arc::new(RwLock::new(None)),
            runtime_handle,
            demux_thread_notify: Arc::new(Notify::new()),
            audio_decode_thread_notify,
            video_decode_thread_notify,
            color_space_converter,
            resampler,
            media_source_flag,
        })
    }
    /// reset all fields to the initial state
    /// this is to make the decoder ready for fresh input
    async fn reset_states(&mut self) {
        self.audio_stream_index = usize::MAX;
        self.video_stream_index = usize::MAX;
        self.cover_stream_index = usize::MAX;
        self.main_stream = MainStream::Audio;
        *self.audio_decoder.write().await = None;

        self.audio_time_base = Rational::new(1, 1);
        self.free_swr_ctx();
        self.cover_pic_data = Arc::new(RwLock::new(None));
        self.video_decode_task_handle = None;
        self.audio_decode_task_handle = None;
        self.decode_exit_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.demux_task_handle = None;
        self.demux_exit_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.end_time_formatted_string = String::new();
        self.end_timestamp
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.format_duration = 0;
        *self.format_input.write().await = None;
        self.hardware_config_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
        *self.video_decoder.write().await = None;
        self.video_frame_rect = [0, 0];
        self.video_time_base = Rational::new(1, 1);
        self.audio_packet_cache_queue.1.drain();
        self.video_packet_cache_queue.1.drain();
        self.audio_frame_cache_queue.1.drain();
        self.video_frame_cache_queue.1.drain();
        self.media_source_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    /// called when user selected a file path to play
    /// init all the details from the file selected
    pub async fn reset_input(&mut self, path: &Path) -> PlayerResult<()> {
        info!("ffmpeg version{}", ffmpeg_the_third::format::version());
        if self.demux_task_handle.is_some() {
            self.stop_background_tasks().await;
            self.reset_states().await;
        }
        let format_input = ffmpeg_the_third::format::input(path)?;
        info!("input construct finished");

        let mut cover_stream = None;
        let mut video_stream = None;
        let mut audio_stream = None;
        for item in format_input.streams() {
            let stream_type = item.parameters().medium();
            if stream_type == ffmpeg_the_third::util::media::Type::Video {
                if item.disposition() == Disposition::ATTACHED_PIC {
                    info!("pic stream was found");
                    cover_stream = Some(ManualProtectedStream(item));
                } else {
                    info!("video stream was found");
                    video_stream = Some(ManualProtectedStream(item));
                }
            } else if stream_type == ffmpeg_the_third::util::media::Type::Audio {
                if audio_stream.is_none() {
                    info!("audio stream was found");
                    audio_stream = Some(ManualProtectedStream(item));
                }
            } else if stream_type == ffmpeg_the_third::util::media::Type::Attachment {
                info!("attachment stream was found");
                cover_stream = Some(ManualProtectedStream(item));
            }
        }
        if audio_stream.is_none() && video_stream.is_none() {
            info!("no valid stream found");
        }
        if let Some(stream) = cover_stream {
            info!("cover stream found");
            self.cover_stream_index = stream.0.index();
        }

        if let Some(stream) = &audio_stream {
            self.audio_stream_index = stream.0.index();
            self.audio_time_base = stream.0.time_base();
            info!("audio time_base==={}", self.audio_time_base);
        }

        if let Some(stream) = &video_stream {
            self.video_stream_index = stream.0.index();
            self.video_time_base = stream.0.time_base();
            info!("video time_base==={}", self.video_time_base);
            if audio_stream.is_none() {
                self.main_stream = MainStream::Video;
            }
        }

        // format_input.duration() can get the precise duration of the media file
        // format_input.duration() number unit is us
        info!("total duration {} us", format_input.duration());
        let format_duration = format_input.duration();
        if format_duration == AV_NOPTS_VALUE {
            self.format_duration = 0;
        } else {
            self.format_duration = format_duration;
        }

        let adur_ts = {
            if format_duration != AV_NOPTS_VALUE {
                if let MainStream::Audio = self.main_stream {
                    format_duration * self.audio_time_base.denominator() as i64
                        / self.audio_time_base.numerator() as i64
                        / 1_000_000
                } else {
                    format_duration * self.video_time_base.denominator() as i64
                        / self.video_time_base.numerator() as i64
                        / 1_000_000
                }
            } else {
                0
            }
        };
        self.end_timestamp
            .store(adur_ts, std::sync::atomic::Ordering::Relaxed);
        self.compute_end_time_str(adur_ts);

        if let Some(audio_stream) = audio_stream {
            let audio_decoder_ctx =
                ffmpeg_the_third::codec::Context::from_parameters(audio_stream.0.parameters())?;

            let mut audio_decoder = audio_decoder_ctx.decoder().audio()?;
            let audio_format = match audio_decoder.format() {
                Sample::None => AVSampleFormat::NONE,

                Sample::U8(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::U8
                    } else {
                        AVSampleFormat::U8P
                    }
                }
                Sample::I16(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::S16
                    } else {
                        AVSampleFormat::S16P
                    }
                }
                Sample::I32(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::S32
                    } else {
                        AVSampleFormat::S32P
                    }
                }
                Sample::I64(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::S64
                    } else {
                        AVSampleFormat::S64P
                    }
                }
                Sample::F32(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::FLT
                    } else {
                        AVSampleFormat::FLTP
                    }
                }
                Sample::F64(t) => {
                    if t == Type::Packed {
                        AVSampleFormat::DBL
                    } else {
                        AVSampleFormat::DBLP
                    }
                }
            };
            if audio_decoder.ch_layout()
                == ChannelLayout::unspecified(audio_decoder.ch_layout().channels())
            {
                audio_decoder.set_ch_layout(ChannelLayout::default_for_channels(
                    audio_decoder.ch_layout().channels(),
                ));
            }
            let resampler = self.alloc_swr_ctx(&audio_decoder, audio_format);
            let mut resampler_guard = self.resampler.write().await;
            *resampler_guard = Some(resampler);
            {
                let mut a_decoder = self.audio_decoder.write().await;
                *a_decoder = Some(ManualProtectedAudioDecoder(audio_decoder));
            }
        }
        if let Some(video_stream) = video_stream {
            let codec_ctx =
                ffmpeg_the_third::codec::Context::from_parameters(video_stream.0.parameters())?;
            let video_decoder = self.choose_decoder_with_hardware_prefer(codec_ctx).await?;
            {
                let mut color_space_converter = self.color_space_converter.write().await;
                color_space_converter.set_params_for_space(
                    video_decoder.color_space(),
                    video_decoder.format(),
                    [video_decoder.width(), video_decoder.height()],
                );
            }

            info!("video decode format{:#?}", video_decoder.format());
            self.video_frame_rect = [video_decoder.width(), video_decoder.height()];
            {
                let mut v_decoder = self.video_decoder.write().await;
                *v_decoder = Some(ManualProtectedVideoDecoder(video_decoder));
            }
        }

        {
            let mut input = self.format_input.write().await;
            *input = Some(ManualProtectedInput(format_input));
        }
        info!("par init finished!!!");
        self.start_process_input().await;
        self.media_source_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    /// the loop of demuxing video file
    async fn packet_demux_process(demux_context: DemuxContext) -> PlayerResult<()> {
        info!("enter demux");
        loop {
            if demux_context
                .demux_exit_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                break;
            }
            /*
            choose to lock the packet vec first stick this in other functions
             */
            if demux_context.audio_packet_sender.len() < 500
                || demux_context.video_packet_sender.len() < 500
            {
                let res = {
                    let mut input = demux_context.format_input.write().await;
                    if let Some(input) = &mut *input {
                        match input.0.packets().next() {
                            Some(Ok((stream, packet))) => Ok((stream.index(), packet)),
                            Some(Err(ffmpeg_the_third::util::error::Error::Eof)) => {
                                info!("demux process hit the end");
                                Err(anyhow::Error::msg("eof"))
                            }
                            None => Err(anyhow::Error::msg("None")),
                            _ => Err(anyhow::Error::msg("Other case")),
                        }
                    } else {
                        Err(anyhow::Error::msg("Other case"))
                    }
                };
                {
                    let audio_stream_idx = demux_context.audio_stream_index;
                    let video_stream_idx = demux_context.video_stream_index;
                    let cover_index = demux_context.cover_stream_index;

                    match res {
                        Ok((stream_idx, packet)) => {
                            if stream_idx == cover_index {
                                if let Some(d) = packet.data() {
                                    let mut cover_pic_data =
                                        demux_context.cover_image_data.write().await;
                                    *cover_pic_data = Some(d.to_vec());
                                }
                            } else if stream_idx == audio_stream_idx {
                                if let Err(e) =
                                    demux_context.audio_packet_sender.send_async(packet).await
                                {
                                    warn!("{}", e);
                                }
                            } else if stream_idx == video_stream_idx {
                                if let Err(e) =
                                    demux_context.video_packet_sender.send_async(packet).await
                                {
                                    warn!("{}", e);
                                }
                            }
                        }
                        Err(e) => {
                            if format!("{}", e) == "eof" {
                                sleep(Duration::from_millis(10)).await;
                            }
                        }
                    }
                }
            } else {
                demux_context.demux_thread_notify.notified().await;
            }
        }
        Ok(())
    }
    ///convert the hardware output frame to middle format YUV420P
    async fn convert_hardware_frame(
        hardware_config: Arc<AtomicBool>,
        video_frame_tmp: Video,
    ) -> Video {
        if hardware_config.load(std::sync::atomic::Ordering::Acquire) {
            unsafe {
                let mut transfered_frame = ffmpeg_the_third::frame::Video::empty();
                if 0 != av_hwframe_transfer_data(
                    transfered_frame.as_mut_ptr(),
                    video_frame_tmp.as_ptr(),
                    0,
                ) {
                    warn!("hardware frame transfer to software frame err");
                }

                transfered_frame.set_pts(video_frame_tmp.pts());
                return transfered_frame;
                // let mut default_frame = Video::empty();
                // {
                //     let mut hardware_frame_converter_guard = hardware_frame_converter.write().await;
                //     if let Some(hardware_frame_converter) = &mut *hardware_frame_converter_guard {
                //         if hardware_frame_converter
                //             .0
                //             .run(&transfered_frame, &mut default_frame)
                //             .is_ok()
                //         {
                //             default_frame.set_pts(transfered_frame.pts());
                //             return default_frame;
                //         }
                //     } else if let Ok(mut ctx) = ffmpeg_the_third::software::converter(
                //         (video_frame_tmp.width(), video_frame_tmp.height()),
                //         transfered_frame.format(),
                //         Pixel::YUV420P,
                //     ) {
                //         info!("transfered_frame format: {:?}", transfered_frame.format());
                //         if ctx.run(&transfered_frame, &mut default_frame).is_ok() {
                //             default_frame.set_pts(transfered_frame.pts());
                //             *hardware_frame_converter_guard = Some(ManualProtectedConverter(ctx));
                //             return default_frame;
                //         }
                //     }
                // }
            }
        }
        video_frame_tmp
    }
    /// the loop of decoding demuxed packet
    async fn video_frame_decode_process(decode_context: VideoDecodeContext) -> PlayerResult<()> {
        info!("enter decode");
        // let mut p = PathBuf::new();
        // let mut graph = Graph::new();
        // if decode_context.video_decoder.read().await.is_some() {
        //     if let Ok(exe_path) = CURRENT_EXE_PATH.as_ref() {
        //         if let Some(exe_folder) = exe_path.parent() {
        //             p = exe_folder.join("app_font.ttf");
        //             if tokio::fs::File::open(&p).await.is_err() {
        //                 if let Ok(mut file) = tokio::fs::File::create_new(&p).await {
        //                     if file.write_all(crate::appui::MAPLE_FONT).await.is_ok() {}
        //                 }
        //             }
        //         }
        //     }

        //     if let Some(font_path_str) = p.to_str() {
        //         let mut font_path_str = font_path_str.replace("\\", "/");
        //         if let Some(idx) = font_path_str.find(':') {
        //             font_path_str.insert(idx, '\\');
        //             unsafe {
        //                 if let Ok(c_str_buffer) = CString::new("buffer") {
        //                     if let Ok(c_str_buffersrc) = CString::new("buffersrc") {
        //                         if let Ok(c_str_buffersrc_args) = CString::new(format!(
        //                             "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1",
        //                             decode_context.video_frame_rect[0],
        //                             decode_context.video_frame_rect[1],
        //                             AVPixelFormat::AV_PIX_FMT_YUV420P as i32,
        //                             decode_context.video_time_base.numerator(),
        //                             decode_context.video_time_base.denominator(),
        //                         )) {
        //                             if let Ok(c_str_drawtext) = CString::new("drawtext") {
        //                                 if let Ok(c_str_draw) = CString::new("draw") {
        //                                     if let Ok(c_str_draw_args) = CString::new(format!(
        //                                         "text='Tiny Player':fontfile={}:fontsize=26:fontcolor=white@0.3:x=w-text_w-10:y=10",
        //                                         font_path_str
        //                                     )) {
        //                                         if let Ok(c_str_buffersink) =
        //                                             CString::new("buffersink")
        //                                         {
        //                                             if let Ok(c_str_sink) = CString::new("sink") {
        //                                                 let buffersrc_filter = avfilter_get_by_name(
        //                                                     c_str_buffer.as_ptr(),
        //                                                 );
        //                                                 // graph free will automatically free filterctx
        //                                                 let mut buffersrc_ctx = null_mut();
        //                                                 let draw_filter = avfilter_get_by_name(
        //                                                     c_str_drawtext.as_ptr(),
        //                                                 );
        //                                                 let mut drawtext_ctx = null_mut();
        //                                                 let buffersink_filter =
        //                                                     avfilter_get_by_name(
        //                                                         c_str_buffersink.as_ptr(),
        //                                                     );
        //                                                 let mut buffersink_ctx = null_mut();
        //                                                 let r = avfilter_graph_create_filter(
        //                                                     &mut buffersrc_ctx,
        //                                                     buffersrc_filter,
        //                                                     c_str_buffersrc.as_ptr(),
        //                                                     c_str_buffersrc_args.as_ptr(),
        //                                                     null_mut(),
        //                                                     graph.as_mut_ptr(),
        //                                                 );
        //                                                 if r < 0 {
        //                                                     info!("create buffer filter err");
        //                                                 }
        //                                                 let r = avfilter_graph_create_filter(
        //                                                     &mut drawtext_ctx,
        //                                                     draw_filter,
        //                                                     c_str_draw.as_ptr(),
        //                                                     c_str_draw_args.as_ptr(),
        //                                                     null_mut(),
        //                                                     graph.as_mut_ptr(),
        //                                                 );
        //                                                 if r < 0 {
        //                                                     info!("create drawtext filter err");
        //                                                 }
        //                                                 let r = avfilter_graph_create_filter(
        //                                                     &mut buffersink_ctx,
        //                                                     buffersink_filter,
        //                                                     c_str_sink.as_ptr(),
        //                                                     null(),
        //                                                     null_mut(),
        //                                                     graph.as_mut_ptr(),
        //                                                 );
        //                                                 if r < 0 {
        //                                                     info!("create buffersink filter err");
        //                                                 }
        //                                                 let r = avfilter_link(
        //                                                     buffersrc_ctx,
        //                                                     0,
        //                                                     drawtext_ctx,
        //                                                     0,
        //                                                 );
        //                                                 if r < 0 {
        //                                                     info!("link src and drawtext err");
        //                                                 }
        //                                                 let r = avfilter_link(
        //                                                     drawtext_ctx,
        //                                                     0,
        //                                                     buffersink_ctx,
        //                                                     0,
        //                                                 );
        //                                                 if r < 0 {
        //                                                     info!("link drawtext and sink err");
        //                                                 }
        //                                                 if graph.validate().is_ok() {
        //                                                     info!(
        //                                                         "graph validate success!dump:\n{}",
        //                                                         graph.dump()
        //                                                     );
        //                                                 }
        //                                             }
        //                                         }
        //                                     }
        //                                 }
        //                             }
        //                         }
        //                     }
        //                 }
        //             }
        //         }
        //     }
        // }
        // let hardware_frame_converter = Arc::new(RwLock::new(None));

        loop {
            if decode_context
                .decode_exit_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                break;
            }
            if decode_context.video_frame_sender.len() < 15 {
                if let Ok(packet) = decode_context.video_packet_recv.recv_async().await {
                    if decode_context.video_packet_recv.len() < 200 {
                        decode_context.demux_thread_notify.notify_one();
                    }
                    let mut v_decoder = decode_context.video_decoder.write().await;
                    // info!("video frame vec len{}", frames.len());
                    if let Some(decoder) = &mut *v_decoder {
                        if decoder.0.send_packet(&packet).is_ok() {
                            loop {
                                let mut video_frame_tmp = ffmpeg_the_third::frame::Video::empty();

                                if decoder.0.receive_frame(&mut video_frame_tmp).is_err() {
                                    break;
                                }
                                let video_frame = TinyDecoder::convert_hardware_frame(
                                    decode_context.hardware_config_flag.clone(),
                                    video_frame_tmp,
                                )
                                .await;
                                // if let Some(mut ctx) = graph.get("buffersrc") {
                                //     if ctx.source().add(&video_frame).is_ok() {
                                //         let mut filtered_frame =
                                //             ffmpeg_the_third::frame::Video::empty();
                                //         if let Some(mut ctx) = graph.get("sink") {
                                //             if ctx.sink().frame(&mut filtered_frame).is_ok() {
                                //                 filter_frame = Some(filtered_frame);
                                //             }
                                //         }
                                //     }
                                // }

                                if let Err(e) = decode_context
                                    .video_frame_sender
                                    .send_async(video_frame)
                                    .await
                                {
                                    warn!("{}", e);
                                }
                            }
                        }
                    }
                }
            } else {
                decode_context.video_decode_thread_notify.notified().await;
            }
        }
        Ok(())
    }
    async fn audio_frame_decode_process(decode_context: AudioDecodeContext) -> PlayerResult<()> {
        loop {
            if decode_context
                .decode_exit_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                break;
            }
            if decode_context.audio_frame_sender.len() < 30 {
                if let Ok(packet) = decode_context.audio_packet_recv.recv_async().await {
                    if decode_context.audio_packet_recv.len() < 200 {
                        decode_context.demux_thread_notify.notify_one();
                    }
                    let mut audio_decoder = decode_context.audio_decoder.write().await;
                    if let Some(decoder) = &mut *audio_decoder {
                        if decoder.0.send_packet(&packet).is_ok() {
                            let mut resampler = decode_context.resampler.write().await;
                            let resampler = resampler.as_mut().context("no resampler allocated")?;
                            loop {
                                let mut audio_frame_tmp = ffmpeg_the_third::frame::Audio::empty();

                                if decoder.0.receive_frame(&mut audio_frame_tmp).is_err() {
                                    break;
                                }
                                let mut resampled_frame = Audio::empty();
                                resampled_frame.set_ch_layout(ChannelLayout::STEREO);
                                resampled_frame.set_rate(AUDIO_SAMPLE_RATE);
                                resampled_frame.set_format(Sample::F32(Type::Packed));
                                resampled_frame.set_pts(audio_frame_tmp.pts());
                                unsafe {
                                    if swr_convert_frame(
                                        resampler.0,
                                        resampled_frame.as_mut_ptr(),
                                        audio_frame_tmp.as_ptr(),
                                    ) != 0
                                    {
                                        warn!("convert audio frame err, but still in decoding!!!");
                                        return Err(anyhow::Error::msg("convert audio frame err"));
                                    }
                                }
                                if let Err(e) = decode_context
                                    .audio_frame_sender
                                    .send_async(resampled_frame)
                                    .await
                                {
                                    warn!("{}", e);
                                }
                            }
                        }
                    }
                }
            } else {
                decode_context.audio_decode_thread_notify.notified().await;
            }
        }
        Ok(())
    }
    /// start the demux and decode task
    async fn start_process_input(&mut self) {
        if let Ok(demux_context) = DemuxContextBuilder::default()
            .audio_stream_index(self.audio_stream_index)
            .video_stream_index(self.video_stream_index)
            .format_input(self.format_input.clone())
            .audio_packet_sender(self.audio_packet_cache_queue.0.clone())
            .video_packet_sender(self.video_packet_cache_queue.0.clone())
            .cover_stream_index(self.cover_stream_index)
            .cover_image_data(self.cover_pic_data.clone())
            .demux_exit_flag(self.demux_exit_flag.clone())
            .demux_thread_notify(self.demux_thread_notify.clone())
            .build()
        {
            self.demux_task_handle = Some(self.runtime_handle.spawn(async move {
                let demux_span = span!(Level::INFO, "demux");
                let _demux_entered = demux_span.enter();
                Self::packet_demux_process(demux_context)
                    .in_current_span()
                    .await
            }));
        } else {
            warn!("build demux context error!");
        }

        if let Ok(decode_context) = VideoDecodeContextBuilder::default()
            .video_decoder(self.video_decoder.clone())
            .video_frame_sender(self.video_frame_cache_queue.0.clone())
            .video_packet_recv(self.video_packet_cache_queue.1.clone())
            .hardware_config_flag(self.hardware_config_flag.clone())
            .decode_exit_flag(self.decode_exit_flag.clone())
            ._video_time_base(self.video_time_base)
            ._video_frame_rect(self.video_frame_rect)
            .video_decode_thread_notify(self.video_decode_thread_notify.clone())
            .demux_thread_notify(self.demux_thread_notify.clone())
            .build()
        {
            self.video_decode_task_handle = Some(self.runtime_handle.spawn(async move {
                let span = span!(Level::INFO, "decode");
                let _entered = span.enter();
                Self::video_frame_decode_process(decode_context)
                    .in_current_span()
                    .await
            }));
        } else {
            warn!("build decode context error!");
        }
        if let Ok(decode_context) = AudioDecodeContextBuilder::default()
            .audio_decoder(self.audio_decoder.clone())
            .audio_frame_sender(self.audio_frame_cache_queue.0.clone())
            .audio_packet_recv(self.audio_packet_cache_queue.1.clone())
            .decode_exit_flag(self.decode_exit_flag.clone())
            .audio_decode_thread_notify(self.audio_decode_thread_notify.clone())
            .demux_thread_notify(self.demux_thread_notify.clone())
            .resampler(self.resampler.clone())
            .build()
        {
            self.audio_decode_task_handle = Some(self.runtime_handle.spawn(async move {
                let span = span!(Level::INFO, "decode");
                let _entered = span.enter();
                Self::audio_frame_decode_process(decode_context)
                    .in_current_span()
                    .await
            }));
        } else {
            warn!("build decode context error!");
        }
    }

    /// seek the input to a selected timestamp
    /// use the ffi function to enable seek all the frames
    /// the ffmpeg_the_third::ffi::AVSEEK_FLAG_ANY flag makes sure
    /// the seek would go as I want, to an exact frame
    pub async fn seek_timestamp_to_decode(&self, ts: i64) {
        let main_stream_idx = {
            if let MainStream::Audio = self.main_stream {
                self.audio_stream_index
            } else {
                self.video_stream_index
            }
        };
        unsafe {
            let mut input = self.format_input.write().await;
            info!("seek timestamp:{}", ts);
            if let Some(input) = &mut *input {
                let res = ffmpeg_the_third::ffi::avformat_seek_file(
                    input.0.as_mut_ptr(),
                    main_stream_idx as i32,
                    i64::MIN,
                    ts,
                    ts,
                    AVSEEK_FLAG_BACKWARD,
                );
                if res != 0 {
                    info!("seek err num:{res}");
                }
                self.flush_decoders().await;
            }
        }

        self.audio_packet_cache_queue.1.drain();
        self.video_packet_cache_queue.1.drain();
        self.audio_frame_cache_queue.1.drain();
        self.video_frame_cache_queue.1.drain();
        info!("seek finished!");
    }
    /// use the file detail to compute the video duration and make str to inform the user
    fn compute_end_time_str(&mut self, end_ts: i64) {
        let sec_num = {
            if let MainStream::Audio = self.main_stream {
                end_ts * self.audio_time_base.numerator() as i64
                    / self.audio_time_base.denominator() as i64
            } else {
                end_ts * self.video_time_base.numerator() as i64
                    / self.video_time_base.denominator() as i64
            }
        };
        let sec = (sec_num % 60) as u8;
        let min_num = sec_num / 60;
        let min = (min_num % 60) as u8;
        let hour_num = min_num / 60;
        let hour = hour_num as u8;
        info!("hour{},min{},sec{}", hour, min, sec);
        if let Ok(time) = time::Time::from_hms(hour, min, sec) {
            if let Ok(formatter) = format_description::parse("[hour]:[minute]:[second]") {
                if let Ok(s) = time.format(&formatter) {
                    self.end_time_formatted_string = s;
                }
            }
        } else {
            info!("end_time_err");
        }
    }

    /// stop demux and decode
    async fn stop_background_tasks(&mut self) {
        self.demux_exit_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.demux_thread_notify.notify_one();

        if let Some(handle) = &mut self.demux_task_handle {
            // if handle.await.is_ok() {
            //     info!("demux thread join success");
            // }
            handle.abort();
            info!("demux thread aborted");
        }

        self.decode_exit_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.video_decode_thread_notify.notify_one();
        if let Some(handle) = &mut self.video_decode_task_handle {
            // if handle.await.is_ok() {
            //     info!("decode thread join success");
            // }
            handle.abort();
        }
        self.audio_decode_thread_notify.notify_one();
        if let Some(handle) = &mut self.audio_decode_task_handle {
            // if handle.await.is_ok() {
            //     info!("decode thread join success");
            // }
            handle.abort();
        }
    }
    /// flush decoder , be called after seek file is done
    async fn flush_decoders(&self) {
        let mut a_decoder = self.audio_decoder.write().await;
        if let Some(a) = &mut *a_decoder {
            a.0.flush();
        }
        let mut v_decoder = self.video_decoder.write().await;
        if let Some(v) = &mut *v_decoder {
            v.0.flush();
        }
    }
}

impl TinyDecoder {
    /// enable hardware accelerate for video decode, currently use d3d12 only on windows
    /// others like vulkan are in developing
    /// fallback to softerware decoder if doesnt support
    async fn choose_decoder_with_hardware_prefer(
        &mut self,
        codec_ctx: codec::Context,
    ) -> PlayerResult<ffmpeg_the_third::decoder::Video> {
        let mut decoder = codec_ctx.decoder().video()?;
        unsafe {
            if let Some(codec) = &decoder.codec() {
                let mut idx = 0;

                let hw_config = loop {
                    let config = avcodec_get_hw_config(codec.as_ptr(), idx);

                    if config.is_null() {
                        break std::ptr::null();
                    }

                    if (*config).device_type == AVHWDeviceType::VULKAN {
                        break config;
                    }

                    idx += 1;
                };

                if hw_config.is_null() {
                    warn!("currently doesn't support hardware accelerate");
                    Ok(decoder)
                } else {
                    let mut hw_device_ctx = null_mut();
                    if 0 != av_hwdevice_ctx_create(
                        &mut hw_device_ctx,
                        (*hw_config).device_type,
                        null(),
                        null_mut(),
                        0,
                    ) {
                        warn!("hw device create err");
                        return Ok(decoder);
                    }
                    (*decoder.as_mut_ptr()).hw_device_ctx = hw_device_ctx;
                    (*decoder.as_mut_ptr()).get_format = Some(get_format_callback);
                    self.hardware_config_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    warn!("hardware decode acceleration is on!");
                    Ok(decoder)
                }
            } else {
                Err(anyhow::Error::msg("err when config hardware acc"))
            }
        }
    }
    fn alloc_swr_ctx(
        &self,
        audio_decoder: &ffmpeg_the_third::decoder::Audio,
        audio_format: AVSampleFormat,
    ) -> ManualProtectedResampler {
        unsafe {
            let mut swr_ctx = null_mut();
            let r = swr_alloc_set_opts2(
                &mut swr_ctx,
                &AV_CHANNEL_LAYOUT_STEREO,
                ffmpeg_the_third::ffi::AVSampleFormat::FLT,
                AUDIO_SAMPLE_RATE as i32,
                audio_decoder.ch_layout().as_ptr(),
                audio_format,
                audio_decoder.rate() as i32,
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
            ManualProtectedResampler(swr_ctx)
        }
    }
    fn free_swr_ctx(&self) {
        let mut resampler = self.resampler.blocking_write();
        if let Ok(ctx) = resampler.as_mut().context("no resampler") {
            unsafe {
                swr_free(&mut ctx.0);
            }
        }
    }
}
unsafe extern "C" fn get_format_callback(
    _ctx: *mut AVCodecContext,
    fmt: *const AVPixelFormat,
) -> AVPixelFormat {
    unsafe {
        let mut i = 0;
        loop {
            if *fmt.add(i) != AVPixelFormat::NONE {
                let current_fmt = *fmt.add(i);

                if current_fmt == AVPixelFormat::VULKAN {
                    return current_fmt;
                }
            } else {
                break;
            }
            i += 1;
        }

        *fmt
    }
}
impl Drop for TinyDecoder {
    /// handle some struct that have to be free manually
    fn drop(&mut self) {
        self.demux_exit_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.decode_exit_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.demux_thread_notify.notify_waiters();
        self.audio_decode_thread_notify.notify_waiters();
        self.video_decode_thread_notify.notify_waiters();
        let demux_task_handle = self.demux_task_handle.take();
        let video_decode_task_handle = self.video_decode_task_handle.take();
        let audio_decode_task_handle = self.audio_decode_task_handle.take();
        self.runtime_handle.spawn(async move {
            demux_task_handle
                .context("join demux thread err")?
                .await??;
            video_decode_task_handle
                .context(anyhow::Error::msg("join decode thread err"))?
                .await??;
            audio_decode_task_handle
                .context("join decode thread err")?
                .await??;
            info!("demux and decode thread exit gracefully");
            PlayerResult::Ok(())
        });
        self.free_swr_ctx();
    }
}
#[derive(Builder)]
struct DemuxContext {
    pub audio_stream_index: usize,
    pub video_stream_index: usize,
    pub cover_stream_index: usize,
    pub format_input: Arc<RwLock<Option<ManualProtectedInput>>>,
    pub audio_packet_sender: Sender<Packet>,
    pub video_packet_sender: Sender<Packet>,
    pub cover_image_data: Arc<RwLock<Option<Vec<u8>>>>,
    pub demux_exit_flag: Arc<AtomicBool>,
    pub demux_thread_notify: Arc<Notify>,
}

#[derive(Builder)]
struct VideoDecodeContext {
    pub video_decoder: Arc<RwLock<Option<ManualProtectedVideoDecoder>>>,
    pub video_packet_recv: Receiver<Packet>,
    pub video_frame_sender: Sender<Video>,
    pub hardware_config_flag: Arc<AtomicBool>,
    pub decode_exit_flag: Arc<AtomicBool>,
    pub _video_time_base: Rational,
    pub _video_frame_rect: [u32; 2],
    pub demux_thread_notify: Arc<Notify>,
    pub video_decode_thread_notify: Arc<Notify>,
}
#[derive(Builder)]
struct AudioDecodeContext {
    pub audio_decoder: Arc<RwLock<Option<ManualProtectedAudioDecoder>>>,
    pub audio_packet_recv: Receiver<Packet>,
    pub audio_frame_sender: Sender<Audio>,
    pub decode_exit_flag: Arc<AtomicBool>,
    pub demux_thread_notify: Arc<Notify>,
    pub audio_decode_thread_notify: Arc<Notify>,
    pub resampler: Arc<RwLock<Option<ManualProtectedResampler>>>,
}
