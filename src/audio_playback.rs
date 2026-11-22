//! The audio_play module manages the audio playback

use std::sync::Arc;

use anyhow::Context;
use cpal::{
    OutputCallbackInfo, Stream, StreamConfig, default_host,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossfire::{AsyncRx, mpsc::Array};
use ffmpeg_the_third::frame::Audio;
use tracing::warn;

use crate::{PlayerResult, appui::AtomicF32};
pub const AUDIO_SAMPLE_RATE: u32 = 48000;
pub const AUDIO_CHANNELS: u32 = 2;
// pub struct AudioPlayer {
//     #[allow(unused)]
//     device_sink: MixerDeviceSink,
//     player: Arc<Player>,
// }
// impl AudioPlayer {
//     pub fn new() -> PlayerResult<Self> {
//         let channel_count = NonZero::new(AUDIO_CHANNELS as u16).context("construct nonzero err")?;
//         let sample_rate = SampleRate::new(AUDIO_SAMPLE_RATE).context("construct SampleRate err")?;
//         let host = default_host();

//         let device = host
//             .default_output_device()
//             .context("get cpal output device err")?;
//         let device_sink = rodio::DeviceSinkBuilder::default()
//             .with_device(device)
//             .with_channels(channel_count)
//             .with_sample_format(rodio::cpal::SampleFormat::F32)
//             .with_sample_rate(sample_rate)
//             .open_stream()?;
//         let sink = Arc::new(rodio::Player::connect_new(device_sink.mixer()));

//         Ok(Self {
//             player: sink,
//             device_sink,
//         })
//     }

//     pub async fn append_source_data(&self, audio_frame: &Audio) -> PlayerResult<()> {
//         let audio_data = bytemuck::cast_slice::<u8, f32>(
//             &audio_frame.data(0)[0..(size_of::<f32>()
//                 * audio_frame.samples()
//                 * audio_frame.ch_layout().channels() as usize)],
//         );
//         let source = rodio::buffer::SamplesBuffer::new(
//             NonZero::new(audio_frame.ch_layout().channels() as u16)
//                 .context("construct nonzero err")?,
//             NonZero::new(audio_frame.rate()).context("construct nonzero err")?,
//             audio_data,
//         );
//         self.player.append(source);
//         Ok(())
//     }

//     pub fn adjust_volume(&self, volumn: f32) {
//         self.player.set_volume(volumn);
//     }
//     pub fn clear_source_queue(&self) {
//         self.player.clear();
//     }
//     pub fn len(&self) -> usize {
//         self.player.len()
//     }
//     pub fn pause(&self) {
//         self.player.pause();
//     }
//     pub fn play(&self) {
//         self.player.play();
//     }
// }
pub struct AudioPlayer {
    output_stream: Stream,
    volume: Arc<AtomicF32>,
}
impl AudioPlayer {
    pub fn new(recv: AsyncRx<Array<Audio>>, volume: Arc<AtomicF32>) -> PlayerResult<Self> {
        let default_host = default_host();
        let device = default_host
            .default_output_device()
            .context("open output stream err")?;

        let config = StreamConfig {
            channels: AUDIO_CHANNELS as u16,
            sample_rate: AUDIO_SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };

        let audio_output_stream_callback = AudioCallback::new(volume.clone());
        let output_stream = device.build_output_stream(
            config,
            audio_output_stream_callback.into_closure(recv),
            |e| warn!("{}", e),
            None,
        )?;
        Ok(Self {
            output_stream,
            volume,
        })
    }
    pub fn adjust_volume(&self, volume: f32) {
        self.volume.store(volume);
    }
    pub fn pause(&self) {
        if let Err(e) = self.output_stream.pause() {
            warn!("{}", e);
        }
    }
    pub fn play(&self) {
        if let Err(e) = self.output_stream.play() {
            warn!("{}", e);
        }
    }
}

struct AudioCallback {
    unfinished_frame: Audio,
    consumed_idx: usize,
    volume: Arc<AtomicF32>,
}
impl AudioCallback {
    pub fn new(volume: Arc<AtomicF32>) -> Self {
        Self {
            unfinished_frame: Audio::empty(),
            consumed_idx: 0,
            volume,
        }
    }
    fn into_closure(
        mut self,
        frame_recv: AsyncRx<Array<Audio>>,
    ) -> impl FnMut(&mut [f32], &OutputCallbackInfo) + Send + 'static {
        move |target_buf, _info| {
            let unfinished_buf = bytemuck::cast_slice::<_, f32>(
                &self.unfinished_frame.data(0)[0..AUDIO_CHANNELS as usize
                    * self.unfinished_frame.samples()
                    * size_of::<f32>()],
            );
            let target_len = target_buf.len();
            let mut filled_size = 0;
            while unfinished_buf.len() > self.consumed_idx && filled_size < target_len {
                let smaller_size =
                    (target_len - filled_size).min(unfinished_buf.len() - self.consumed_idx);
                target_buf[filled_size..(filled_size + smaller_size)].copy_from_slice(
                    &unfinished_buf[self.consumed_idx..(self.consumed_idx + smaller_size)],
                );
                self.consumed_idx += smaller_size;
                filled_size += smaller_size;
            }
            let volume = self.volume.load();
            while filled_size < target_len
                && let Ok(mut frame) = frame_recv.try_recv()
            {
                let frame_samples = frame.samples();
                let audio_frame = bytemuck::cast_slice_mut::<_, f32>(
                    &mut frame.data_mut(0)
                        [0..AUDIO_CHANNELS as usize * frame_samples * size_of::<f32>()],
                );
                for item in audio_frame.iter_mut() {
                    (*item) *= volume;
                }
                if target_len - filled_size >= audio_frame.len() {
                    target_buf[filled_size..(filled_size + audio_frame.len())]
                        .copy_from_slice(audio_frame);
                    filled_size += audio_frame.len();
                } else {
                    target_buf[filled_size..]
                        .copy_from_slice(&audio_frame[0..(target_len - filled_size)]);
                    self.unfinished_frame = frame;
                    self.consumed_idx = target_len - filled_size;
                    filled_size = target_len;
                }
            }
            if filled_size < target_len {
                target_buf[filled_size..].fill(0.0);
            }
        }
    }
}
