use std::{num::NonZero, sync::Arc};

use anyhow::Context;
use rodio::{
    MixerDeviceSink, Player, SampleRate,
    cpal::{default_host, traits::HostTrait},
};

use crate::PlayerResult;
pub const AUDIO_SAMPLE_RATE: u32 = 48000;
pub const AUDIO_CHANNELS: u32 = 2;
pub struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    sink: Arc<Player>,
}
impl AudioPlayer {
    pub fn new() -> PlayerResult<Self> {
        let channel_count = NonZero::new(AUDIO_CHANNELS as u16).context("construct nonzero err")?;
        let sample_rate = SampleRate::new(AUDIO_SAMPLE_RATE).context("construct SampleRate err")?;
        let host = default_host();

        let device = host
            .default_output_device()
            .context("get cpal output device err")?;
        let device_sink = rodio::DeviceSinkBuilder::default()
            .with_device(device)
            .with_channels(channel_count)
            .with_sample_format(rodio::cpal::SampleFormat::F32)
            .with_sample_rate(sample_rate)
            .open_stream()?;
        let sink = Arc::new(rodio::Player::connect_new(device_sink.mixer()));

        Ok(Self {
            sink,
            _device_sink: device_sink,
        })
    }

    pub async fn play_raw_data_from_audio_frame(
        sink: &Player,
        audio_frame: ffmpeg_the_third::frame::Audio,
    ) -> PlayerResult<()> {
        let audio_data = bytemuck::cast_slice::<u8, f32>(
            &audio_frame.data(0)[0..(size_of::<f32>()
                * audio_frame.samples()
                * audio_frame.ch_layout().channels() as usize)],
        );
        let source = rodio::buffer::SamplesBuffer::new(
            NonZero::new(audio_frame.ch_layout().channels() as u16)
                .context("construct nonzero err")?,
            NonZero::new(audio_frame.rate()).context("construct nonzero err")?,
            audio_data,
        );
        sink.append(source);
        Ok(())
    }

    pub fn change_volumn(&self, volumn: f32) {
        self.sink.set_volume(volumn);
    }
    pub fn source_queue_skip_to_end(&self) {
        self.sink.clear();
    }
    pub fn pause(&self) {
        self.sink.pause();
    }
    pub fn play(&self) {
        self.sink.play();
    }
    pub fn sink(&self) -> Arc<Player> {
        self.sink.clone()
    }
}
