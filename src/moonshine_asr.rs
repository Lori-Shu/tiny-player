#![allow(unused)]
use std::{
    ffi::{CStr, CString, c_char, c_float, c_int, c_uint, c_ulonglong, c_void},
    ptr::{null, null_mut},
};

use ffmpeg_the_third::{
    ChannelLayout, ffi::{
        AV_CHANNEL_LAYOUT_MONO, AV_CHANNEL_LAYOUT_STEREO, swr_alloc_set_opts2,
        swr_convert_frame, swr_free, swr_init,
    }, format::Sample, frame::Audio
};
use flume::Sender;
use tracing::{info, warn};

use crate::{
    CURRENT_EXE_PATH, PlayerResult, decode::ManualProtectedResampler,
    present_data_manage::PLAY_SAMPLE_RATE,
};

const MOONSHINE_MODEL_ARCH_TINY_STREAMING: u32 = 2;
const MOONSHINE_HEADER_VERSION: i32 = 20000;
unsafe extern "C" {
    unsafe fn moonshine_load_transcriber_from_files(
        path: *const c_char,
        model_arch: c_uint,
        options: *const c_void,
        options_count: c_ulonglong,
        moonshine_version: c_int,
    ) -> c_int;
    unsafe fn moonshine_free_transcriber(transcriber_handle: c_int);
    unsafe fn moonshine_create_stream(transcriber_handle: c_int, flags: c_uint) -> c_int;
    unsafe fn moonshine_start_stream(transcriber_handle: c_int, stream_handle: c_int) -> c_int;
    unsafe fn moonshine_stop_stream(transcriber_handle: c_int, stream_handle: c_int) -> c_int;
    unsafe fn moonshine_free_stream(transcriber_handle: c_int, stream_handle: c_int) -> c_int;
    unsafe fn moonshine_transcribe_add_audio_to_stream(
        transcriber_handle: c_int,
        stream_handle: c_int,
        new_audio_data: *const c_float,
        audio_length: c_ulonglong,
        sample_rate: c_int,
        flags: c_uint,
    ) -> c_int;
    unsafe fn moonshine_transcribe_stream(
        transcriber_handle: c_int,
        stream_handle: c_int,
        flags: c_uint,
        out_transcript: *mut *mut Transcript,
    ) -> c_int;
}
#[repr(C)]
struct Transcript {
    lines: *const TranscriptLine, /* All lines of the transcript. */
    line_count: c_ulonglong,      /* Number of lines in the transcript.      */
}
#[repr(C)]
struct TranscriptLine {
    /* UTF-8-encoded transcription. */
    text: *const c_char,
    /* The audio data for the current phrase. */
    audio_data: *const c_float,
    /* The number of elements in the audio data array. */
    audio_data_count: c_ulonglong,
    /* Time offset from the start of the array or stream in seconds.  */
    start_time: c_float,
    /* How long the segment currently is in seconds. */
    duration: c_float,
    /* Stable identifier for the line. */
    id: c_ulonglong,
    /* Streaming-only: Zero means the speaker hasn't finished talking in this
     * segment, non-zero means they have. */
    is_complete: c_char,
    /* Streaming-only: Whether the line has been updated since the previous call
     * to transcribe_stream_chunk. */
    is_updated: c_char,
    /* Streaming-only: Whether the line was newly added since the previous call to
     * transcribe_stream_chunk. */
    is_new: c_char,
    /* Streaming-only: Whether the text of the line has changed since the previous
     * call to transcribe_stream_chunk. */
    has_text_changed: c_char,
    /* Whether a speaker ID has been calculated for the line. */
    has_speaker_id: c_char,
    /* The speaker ID for the line. */
    speaker_id: c_ulonglong,
    /* What order the speaker appeared in the current transcript. */
    speaker_index: c_uint,
    /* Streaming-only: The latency of the last transcription in milliseconds. */
    last_transcription_latency_ms: c_uint,
    /* Word-level timestamps. NULL if word_timestamps option is not enabled. */
    words: *const TranscriptWord,
    /* Number of words in the words array. 0 if not enabled. */
    word_count: c_ulonglong,
}
#[repr(C)]
struct TranscriptWord {
    /* UTF-8-encoded word text. */
    text: *const c_char,
    /* Start time in seconds (absolute, from start of audio/stream). */
    start: c_float,
    /* End time in seconds. */
    end: c_float,
    /* Model confidence score, 0.0 to 1.0. */
    confidence: c_float,
}
struct ManualSafeTranscript(*mut Transcript);
unsafe impl Send for ManualSafeTranscript {}

const TRANSCRIBE_SAMPLE_RATE: u32 = 16000;
#[derive(Debug, Clone)]
pub struct Transcriber {
    transcriber_handle: i32,
    stream_handle: i32,
    subtitle_sender: Sender<String>,
    audio_resampler: ManualProtectedResampler,
}
impl Transcriber {
    pub fn new() -> PlayerResult<Self> {
        let subtitle_sender=flume::bounded(5).0;
        let exe_path = CURRENT_EXE_PATH
            .as_ref()
            .map_err(|_e| anyhow::Error::msg("exe_path_as_ref err"))?;
        let exe_dir = exe_path
            .parent()
            .ok_or(anyhow::Error::msg("get parent_dir err"))?;
        let model_path = exe_dir.join("moonshine_tiny_streaming");
        let path_str = model_path
            .to_str()
            .ok_or(anyhow::Error::msg("to str failed"))?;
        let mut path_str = path_str.to_string();
        path_str.push('/');
        let path_c_ctring = CString::new(path_str)?;
        unsafe {
            let mut swr_ctx = null_mut();
            let r = swr_alloc_set_opts2(
                &mut swr_ctx,
                &AV_CHANNEL_LAYOUT_MONO,
                ffmpeg_the_third::ffi::AVSampleFormat::FLT,
                TRANSCRIBE_SAMPLE_RATE as i32,
                &AV_CHANNEL_LAYOUT_STEREO,
                ffmpeg_the_third::ffi::AVSampleFormat::FLT,
                PLAY_SAMPLE_RATE as i32,
                0,
                null_mut(),
            );
            if r < 0 {
                warn!("swr ctx create err");
            }
            let r = swr_init(swr_ctx);
            if r < 0 {
                warn!("swr init err");
            }
            let transcriber_handle = moonshine_load_transcriber_from_files(
                path_c_ctring.as_ptr(),
                MOONSHINE_MODEL_ARCH_TINY_STREAMING,
                null(),
                0,
                MOONSHINE_HEADER_VERSION,
            );
            if transcriber_handle < 0 {
                return Err(anyhow::Error::msg("transcriber_handle load err!"));
            }
            let stream_handle = moonshine_create_stream(transcriber_handle, 0);
            if stream_handle < 0 {
                return Err(anyhow::Error::msg("create moonshine stream err!"));
            }
            if moonshine_start_stream(transcriber_handle, stream_handle) != 0 {
                return Err(anyhow::Error::msg("moonshine_start_stream err"));
            }

            Ok(Self {
                transcriber_handle,
                stream_handle,
                subtitle_sender,
                audio_resampler: ManualProtectedResampler(swr_ctx),
            })
        }
    }
    pub async fn push_audio_frame(&mut self, frame: Audio, model: UsedModel) -> PlayerResult<()> {
        if model == UsedModel::English {}
        info!("audio frame channel{:?},format{:?},rate{}",frame.ch_layout(),frame.format(),frame.rate());
        unsafe {
            let mut to_recognize_frame = Audio::empty();
            to_recognize_frame.set_format(Sample::F32(ffmpeg_the_third::format::sample::Type::Packed));
            to_recognize_frame.set_ch_layout(ChannelLayout::MONO);
            to_recognize_frame.set_rate(TRANSCRIBE_SAMPLE_RATE);

            let err_num =
                swr_convert_frame(self.audio_resampler.0, to_recognize_frame.as_mut_ptr(), frame.as_ptr());
            if err_num < 0 {
                warn!("audio frame convert err: {}", err_num);
            }
            if moonshine_transcribe_add_audio_to_stream(
                self.transcriber_handle,
                self.stream_handle,
                bytemuck::cast_slice::<_, f32>(
                    &to_recognize_frame.data(0)[0..(to_recognize_frame.samples()
                        * to_recognize_frame.ch_layout().channels() as usize
                        * 4)],
                )
                .as_ptr(),
                (to_recognize_frame.samples() * to_recognize_frame.ch_layout().channels() as usize)
                    as u64,
                TRANSCRIBE_SAMPLE_RATE as i32,
                0,
            ) != 0
            {
                return Err(anyhow::Error::msg(
                    "moonshine_transcribe_add_audio_to_stream err",
                ));
            }
            let out_transcript = {
                let mut out_transcript = null_mut();
                if moonshine_transcribe_stream(
                    self.transcriber_handle,
                    self.stream_handle,
                    0,
                    &mut out_transcript,
                ) != 0
                {
                    return Err(anyhow::Error::msg("moonshine_transcribe_stream err"));
                }
                ManualSafeTranscript(out_transcript)
            };
            info!("after ManualSafeTranscript");
            for line_idx in 0..(*out_transcript.0).line_count {
                let transcript_line = &*(*out_transcript.0).lines.add(line_idx as usize);
                if transcript_line.is_complete == 0 {
                    let line_text = transcript_line.text;
                    let text = CStr::from_ptr(line_text).to_str()?.to_string();
                    self.subtitle_sender.send_async(text).await?;
                }
            }
        }
        Ok(())
    }
}
impl Drop for Transcriber {
    fn drop(&mut self) {
        unsafe {
            swr_free(&mut self.audio_resampler.0);
            moonshine_stop_stream(self.transcriber_handle, self.stream_handle);
            moonshine_free_stream(self.transcriber_handle, self.stream_handle);
            moonshine_free_transcriber(self.transcriber_handle);
        }
    }
}
#[derive(Debug, PartialEq, Clone)]
pub enum UsedModel {
    None,
    Chinese,
    English,
}
