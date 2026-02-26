use std::sync::Arc;

use rodio::Sink;

use crate::PlayerResult;

pub struct AudioPlayer {
    sink: Arc<rodio::Sink>,
    _stream: rodio::OutputStream,
    current_volumn: f32,
}
impl AudioPlayer {
    pub fn new() -> PlayerResult<Self> {
        let stream = rodio::OutputStreamBuilder::open_default_stream()?;
        let sink = Arc::new(rodio::Sink::connect_new(stream.mixer()));

        Ok(Self {
            sink,
            _stream: stream,
            current_volumn: 1.0,
        })
    }

    pub async fn play_raw_data_from_audio_frame(
        sink: &Sink,
        audio_frame: ffmpeg_the_third::frame::Audio,
    ) {
        let audio_data = bytemuck::cast_slice::<u8, f32>(audio_frame.data(0));
        let audio_data =
            &audio_data[0..audio_frame.samples() * audio_frame.ch_layout().channels() as usize];
        let source = rodio::buffer::SamplesBuffer::new(
            audio_frame.ch_layout().channels() as u16,
            audio_frame.rate(),
            audio_data,
        );
        sink.append(source);
    }

    pub fn change_volumn(&mut self, volumn: f32) {
        self.sink.set_volume(volumn);
        self.current_volumn = volumn;
    }
    pub fn source_queue_skip_to_end(&mut self) {
        self.sink.clear();
    }
    pub fn pause(&self) {
        self.sink.pause();
    }
    pub fn play(&self) {
        self.sink.play();
    }

    pub fn current_volumn(&self) -> &f32 {
        &self.current_volumn
    }
    pub fn sink(&self) -> Arc<Sink> {
        self.sink.clone()
    }

    pub fn len(&self) -> usize {
        self.sink.len()
    }
}
