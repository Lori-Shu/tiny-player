use burn::tensor::{
    self, Int, Shape, Tensor, TensorData, activation::log_softmax, backend::Backend,
};
use rustfft::Fft;
use std::{iter, ops::Div, sync::Arc};
use tracing::warn;

use crate::{
    PlayerResult,
    whisper_burn::{
        audio::{max_waveform_samples, prep_audio},
        beam::{self, BeamNode},
        model::Whisper,
        token::{self, Gpt2Tokenizer},
    },
};
const PADDING: usize = 200; //ADJUST THIS IF CHINKS ARE REPEATING THEMSELVES ENDLESSLY
pub fn waveform_to_text<B: Backend>(
    whisper: &Whisper<B>,
    bpe: &Gpt2Tokenizer,
    waveform: Vec<f32>,
    sample_rate: usize,
    streaming_mode: bool,
    fft: Arc<dyn Fft<f32>>,
    mel_filters: &Tensor<B, 2>,
    hann_window: &Tensor<B, 1>,
    initial_tokens: &Vec<usize>,
    end_token: &usize,
) -> token::Result<(String, Vec<usize>)> {
    let device = mel_filters.device();

    let n_ctx_max_encoder = whisper.encoder_ctx_size();

    let n_waveform_samples_per_window = max_waveform_samples(n_ctx_max_encoder - PADDING);
    // info!("n_waveform_samples_per_window{}",n_waveform_samples_per_window);
    let mel_iter = waveform_to_mel_tensor(
        waveform,
        sample_rate,
        n_waveform_samples_per_window,
        device,
        mel_filters,
        fft,
        hann_window,
    )?;

    let mut text = String::new();
    let mut tokens = Vec::new();

    //IN THE FOLLOWING CODE, WE WILL PRETTY MUCH ALWAYS ITERATE JUST ONCE, SINCE WE ARE SENDING SUCH SHORT CLIPS OF AUDIO. THIS MEANS FIND CHUNK OVERLAP IS NOT NECESSARY BUT CAN LEAVE IT FOR THE FUTURE
    for (_i, mel) in mel_iter.enumerate() {
        // info!("before mels_to_text");
        let (_new_text, new_tokens) = mels_to_text(
            whisper,
            bpe,
            mel,
            PADDING,
            initial_tokens,
            end_token,
            streaming_mode,
        )?;
        // info!("before find_chunk_overlap");
        // if let Some((prev_index, curr_index)) =
        //     find_chunk_overlap(&tokens[..], &new_tokens[..], 40, 3)
        // {
        //     tokens.truncate(prev_index);
        //     tokens.extend(&new_tokens[curr_index..]);
        // } else {
        //     tokens.extend(new_tokens);
        // }
        // info!("before bpe.decode");
        tokens.extend(new_tokens);
        text = bpe.decode(&tokens[..], true)?;
    }

    Ok((text, tokens))
}

fn waveform_to_mel_tensor<B: Backend>(
    waveform: Vec<f32>,
    sample_rate: usize,
    window_length_samples: usize,
    device: B::Device,
    mel_filters: &Tensor<B, 2>,
    fft: Arc<dyn Fft<f32>>,
    hann_window: &Tensor<B, 1>,
) -> PlayerResult<impl Iterator<Item = Tensor<B, 3>>> {
    let chunk_overlap = sample_rate;
    let n_samples_per_tensor = window_length_samples;
    let shift = n_samples_per_tensor.saturating_sub(chunk_overlap).max(1);
    let iter_len = waveform.len().saturating_sub(1).div(shift) + 1;
    Ok((0..iter_len).into_iter().map(move |i| {
        let start = i * shift;
        let end = (start + n_samples_per_tensor).min(waveform.len());

        let slice = &waveform[start..end];

        let waveform: Tensor<B, 1> = Tensor::from_data(
            tensor::TensorData::new(slice.to_vec(), [slice.len()]),
            &device,
        );
        // info!("before prep_audio");
        if let Ok(mels) = prep_audio(waveform.unsqueeze(), mel_filters, fft.clone(), hann_window) {
            mels
        } else {
            panic!("prep audio err!");
        }
    }))
}

#[derive(Clone)]
pub struct BeamSearchToken {
    token: usize,
    _log_prob: f64,
}

fn mels_to_text<B: Backend>(
    whisper: &Whisper<B>,
    bpe: &Gpt2Tokenizer,
    mels: Tensor<B, 3>,
    padding: usize,
    initial_tokens: &Vec<usize>,
    end_token: &usize,
    _streaming_mode: bool,
) -> PlayerResult<(String, Vec<usize>)> {
    let device = mels.device();
    // info!("log debug point0");
    let n_ctx_max_encoder = whisper.encoder_ctx_size();
    // info!("log debug point1");
    let _n_ctx_max_decoder = whisper.decoder_ctx_size();
    // info!("log debug point2");
    let [_n_channel, n_mel, n_ctx] = mels.dims();
    if n_ctx + padding > n_ctx_max_encoder {
        warn!(
            "Audio has length of {} which exceeds maximum length {}. It will be clipped.",
            n_ctx + padding,
            n_ctx_max_encoder
        );
    }

    // info!("log debug point6");
    // the zero padding helps whisper determine end of text

    let mels = Tensor::cat(
        vec![
            mels.slice([0..1, 0..n_mel, 0..(n_ctx).min(n_ctx_max_encoder - padding)]),
            Tensor::zeros([1, n_mel, padding], &device),
        ],
        2,
    );

    // info!("before forward_encoder");
    let encoder_output = whisper.forward_encoder(mels);
    type BeamNode = beam::BeamNode<BeamSearchToken>;
    let initial_tokens = BeamNode {
        seq: initial_tokens
            .into_iter()
            .map(|tok| BeamSearchToken {
                token: *tok,
                _log_prob: 0.0,
            })
            .collect(),
        log_prob: 0.0,
    };

    let neg_infty = -f32::INFINITY;

    let vocab_size = bpe.vocab_size();
    let special_tokens_maskout: Vec<f32> = (0..vocab_size)
        .into_iter()
        .map(|token| {
            if bpe.is_special(token) {
                neg_infty
            } else {
                0.0
            }
        })
        .collect();
    //special_tokens_maskout[end_token] = 1.0;
    // info!("log debug point7");
    let special_tokens_maskout: Tensor<B, 1> = Tensor::from_data(
        TensorData::new(special_tokens_maskout, Shape::from([vocab_size])),
        &device,
    );
    // info!("log debug point8");

    let max_tokens = 25; // 2秒正常语速很难超过15-20个词，25是安全的激进上限
    let end_token_id = *end_token;

    let beamsearch_is_finished = move |toks: &[BeamSearchToken]| {
        if toks.is_empty() {
            return false;
        }

        let last_tok = toks.last().unwrap().token;

        // 1. 标准结束符判断
        if last_tok == end_token_id {
            return true;
        }

        // 2. 强力长度熔断：2秒音频不可能吐出长篇大论
        if toks.len() >= max_tokens {
            return true;
        }

        // 3. 实时重复检测 (N-gram Blocking)
        // 如果最近 3 个词在序列中已经出现过，说明陷入了循环，直接切断
        if toks.len() > 6 {
            let window_size = 3;
            let last_window = &toks[toks.len() - window_size..];
            let previous_content = &toks[..toks.len() - window_size];

            // 检查最后的 window 是否在前面完整出现过
            if previous_content
                .windows(window_size)
                .any(|w| w.iter().zip(last_window).all(|(a, b)| a.token == b.token))
            {
                return true; // 触发重复熔断
            }
        }

        // 4. 单 Token 连续重复判断 (例如 ".........")
        if toks.len() >= 3 {
            let last_three = &toks[toks.len() - 3..];
            if last_three.iter().all(|t| t.token == last_tok) {
                // 排除某些合法的特殊 token（如时间戳）后，连续3个相同 token 视为幻觉
                if last_tok < 50257 {
                    return true;
                }
            }
        }

        false
    };

    let beam_size = 1;
    let max_depth = 12;
    // info!("log debug point9");

    let tokens: Vec<_> = beam::beam_search(
        vec![initial_tokens],
        whisper,
        &encoder_output,
        &special_tokens_maskout,
        beamsearch_is_finished,
        beam_size,
        max_depth,
    )
    .into_iter()
    .map(|btok| btok.token)
    .collect();
    // info!("bpe decode");
    let text = bpe
        .decode(&tokens[..], false)
        .map_err(|e| anyhow::Error::msg(e))?;
    // info!("finish mels_to_text");
    return Ok((text, tokens));
}
pub fn beamsearch_next<B: Backend>(
    beams: &[BeamNode<BeamSearchToken>],
    whisper: &Whisper<B>,
    encoder_output: &Tensor<B, 3>,
    special_tokens_maskout: &Tensor<B, 1>,
    beam_size: usize,
) -> Vec<Vec<(BeamSearchToken, f64)>> {
    let device = encoder_output.device();
    let n_beams = beams.len();

    let max_seq_len = beams.iter().map(|beam| beam.seq.len()).max().unwrap_or(0);
    let mut flattened_tokens = Vec::with_capacity(n_beams * max_seq_len);

    for beam in beams {
        let len = beam.seq.len();
        flattened_tokens.extend(beam.seq.iter().map(|btok| btok.token as i32));
        if len < max_seq_len {
            flattened_tokens.extend(iter::repeat(0).take(max_seq_len - len));
        }
    }

    let token_tensor = Tensor::<B, 2, Int>::from_data(
        TensorData::new(flattened_tokens, Shape::from([n_beams, max_seq_len])),
        &device,
    );

    // 2. Decoder 前向传播
    let logits =
        whisper.forward_decoder(token_tensor, encoder_output.clone().repeat_dim(0, n_beams));

    let mut last_token_logits = logits.slice([0..n_beams, (max_seq_len - 1)..max_seq_len]);

    let penalty_factor = 1.2_f32;

    let vocab_size = last_token_logits.dims()[2];
    let mut penalty_map = vec![1.0_f32; n_beams * vocab_size];

    for (i, beam) in beams.iter().enumerate() {
        let lookback = 10;
        let recent_tokens = beam.seq.iter().rev().take(lookback);
        for b_tok in recent_tokens {
            let tid = b_tok.token;
            if tid >= 50257 {
                continue;
            }
            penalty_map[i * vocab_size + tid] = penalty_factor;
        }
    }

    let penalty_tensor = Tensor::<B, 3>::from_data(
        TensorData::new(penalty_map, Shape::from([n_beams, 1, vocab_size])),
        &device,
    );

    let is_positive = last_token_logits.clone().greater_equal_elem(0.0);
    let positive_logits = last_token_logits.clone().div(penalty_tensor.clone());
    let negative_logits = last_token_logits.clone().mul(penalty_tensor);

    last_token_logits = positive_logits.mask_where(is_positive, negative_logits);
    let masked_logits = if max_seq_len <= 5 {
        last_token_logits + special_tokens_maskout.clone().unsqueeze::<3>()
    } else {
        last_token_logits
    };

    let log_probs = log_softmax(masked_logits, 2);

    let top_k = beam_size * 2;
    let (top_k_values_tensor, top_k_indices_tensor) = log_probs.topk_with_indices(top_k, 2);

    let top_k_values = top_k_values_tensor
        .flatten::<1>(0, 2)
        .into_data()
        .convert::<f32>()
        .into_vec::<f32>()
        .expect("Values sync failed");

    let top_k_ids = top_k_indices_tensor
        .flatten::<1>(0, 2)
        .into_data()
        .convert::<i32>()
        .into_vec::<i32>()
        .expect("Indices sync failed");

    let mut continuations = Vec::with_capacity(n_beams);
    for i in 0..n_beams {
        let mut beam_conts = Vec::with_capacity(top_k);
        for j in 0..top_k {
            let offset = i * top_k + j;
            let token_id = top_k_ids[offset] as usize;
            let log_prob = top_k_values[offset] as f64;

            beam_conts.push((
                BeamSearchToken {
                    token: token_id,
                    _log_prob: log_prob,
                },
                beams[i].log_prob + log_prob,
            ));
        }
        continuations.push(beam_conts);
    }

    continuations
}

//HELPERS
fn _find_chunk_overlap(
    prev_tokens: &[usize],
    curr_tokens: &[usize],
    max_n_offsets: usize,
    min_n_overlaps: usize,
) -> Option<(usize, usize)> {
    let mut max_overlap = 0;
    let mut max_overlap_indices = (0, 0);
    let n_offsets = prev_tokens.len().min(curr_tokens.len()).min(max_n_offsets);

    for offset in 0..n_offsets {
        let prev_start_index = prev_tokens.len() - 1 - offset;
        let mut overlap_iter = prev_tokens
            .iter()
            .skip(prev_start_index)
            .zip(curr_tokens.iter())
            .enumerate()
            .filter(|(_, (old, new))| **old == **new);

        let n_overlap = overlap_iter.clone().count();
        if n_overlap > max_overlap {
            max_overlap = n_overlap;

            let curr_overlap_index = overlap_iter.next().unwrap().0;
            let prev_overlap_index = prev_start_index + curr_overlap_index;
            max_overlap_indices = (prev_overlap_index, curr_overlap_index)
        }
    }

    if max_overlap >= min_n_overlaps {
        Some(max_overlap_indices)
    } else {
        None
    }
}
