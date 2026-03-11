#![allow(unused)]

use burn::{Tensor, prelude::Backend};

use crate::whisper_burn::{
    model::Whisper,
    transcribe::{BeamSearchToken, beamsearch_next},
};
#[derive(Clone)]
pub struct BeamNode<T: Clone> {
    pub seq: Vec<T>,
    pub log_prob: f64,
}

pub fn beam_search<G, B: Backend>(
    initial_beams: Vec<BeamNode<BeamSearchToken>>,
    whisper: &Whisper<B>,
    encoder_output: &Tensor<B, 3>,
    special_tokens_maskout: &Tensor<B, 1>,
    is_finished: G,
    beam_size: usize,
    max_depth: usize,
) -> Vec<BeamSearchToken>
where
    G: Fn(&[BeamSearchToken]) -> bool + Clone,
{
    let mut active_beams = initial_beams;
    let mut best_finished_beam: Option<BeamNode<BeamSearchToken>> = None;

    for _i in 0..max_depth {
        // 1. 如果没有活跃路径了，直接跳出循环
        if active_beams.is_empty() {
            break;
        }

        // 2. 执行步进，获取 (活跃路径, 新生成的结束路径)
        let (next_active, newly_finished) = beam_search_step(
            active_beams,
            whisper,
            encoder_output,
            special_tokens_maskout,
            is_finished.clone(),
            beam_size,
        );

        // 3. 更新全局最佳结束路径
        // 我们只保留得分最高（log_prob 最大）的结束序列
        if let Some(top_newly_finished) = newly_finished
            .into_iter()
            .max_by(|a, b| a.log_prob.partial_cmp(&b.log_prob).unwrap())
        {
            match &best_finished_beam {
                Some(current_best) if top_newly_finished.log_prob > current_best.log_prob => {
                    best_finished_beam = Some(top_newly_finished);
                }
                None => {
                    best_finished_beam = Some(top_newly_finished);
                }
                _ => {} // 保持现状
            }
        }

        // 4. 更新活跃路径，进入下一轮
        active_beams = next_active;

        // 5. 可选：早停逻辑
        // 如果当前最佳结束路径的得分已经远高于所有活跃路径，可以提前结束
        if let Some(ref finished) = best_finished_beam {
            if let Some(best_active) = active_beams
                .iter()
                .max_by(|a, b| a.log_prob.partial_cmp(&b.log_prob).unwrap())
            {
                // 如果活跃路径即使加上未来的理想增益也追不上已结束路径，就停掉
                if finished.log_prob > best_active.log_prob + 5.0 {
                    // 5.0 是启发式阈值
                    break;
                }
            }
        }
    }

    // 最终结算：优先返回最佳的已结束路径；如果没有，返回当前表现最好的活跃路径
    let final_node = best_finished_beam.or_else(|| {
        active_beams
            .into_iter()
            .max_by(|a, b| a.log_prob.partial_cmp(&b.log_prob).unwrap())
    });

    final_node.map(|x| x.seq).unwrap_or_else(Vec::new)
}

pub fn beam_search_step<G, B: Backend>(
    beams: Vec<BeamNode<BeamSearchToken>>,
    whisper: &Whisper<B>,
    encoder_output: &Tensor<B, 3>,
    special_tokens_maskout: &Tensor<B, 1>,
    is_finished: G,
    beam_size: usize,
) -> (
    Vec<BeamNode<BeamSearchToken>>,
    Vec<BeamNode<BeamSearchToken>>,
)
where
    G: Fn(&[BeamSearchToken]) -> bool,
{
    // 1. 只对尚未结束的 beams 进行推理（这里是提速的关键！）
    let continuations = beamsearch_next(
        &beams,
        whisper,
        encoder_output,
        special_tokens_maskout,
        beam_size,
    );

    let mut next_active_candidates = Vec::with_capacity(beams.len() * beam_size);
    let mut newly_finished = Vec::with_capacity(beam_size);

    // 2. 将产生的新词立即加入序列并判断
    for (parent_node, beam_conts) in beams.into_iter().zip(continuations) {
        for (tok, log_prob) in beam_conts {
            let mut new_seq = parent_node.seq.clone();
            new_seq.push(tok);

            let new_node = BeamNode {
                seq: new_seq,
                log_prob, // 假设 beamsearch_next 已经累加了 log_prob
            };

            // 立即检查新产生的 Token 是否触发结束
            if is_finished(&new_node.seq) {
                newly_finished.push(new_node);
            } else {
                next_active_candidates.push(new_node);
            }
        }
    }

    // 3. 仅从活跃候选词中选出 Top-K 进入下一轮
    let top_active = _get_top_elements(&next_active_candidates, |b| b.log_prob, beam_size)
        .into_iter()
        .cloned()
        .collect();

    (top_active, newly_finished)
}

fn _get_top_elements<T>(elems: &[T], score: impl Fn(&T) -> f64, num: usize) -> Vec<&T> {
    let mut top_elems = Vec::with_capacity(num);
    let mut scores = Vec::with_capacity(num);

    for elem in elems {
        let score = score(elem);

        // most common scenario
        if top_elems.len() == num {
            if score < scores[0] {
                continue;
            }
        }

        if let Some((idx, _)) = scores.iter().enumerate().find(|(_, s)| **s >= score) {
            top_elems.insert(idx, elem);
            scores.insert(idx, score);
        } else {
            top_elems.push(elem);
            scores.push(score);
        }

        if top_elems.len() > num {
            top_elems.remove(0);
            scores.remove(0);
        }
    }

    top_elems
}
