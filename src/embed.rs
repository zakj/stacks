use model2vec_rs::model::StaticModel;

use crate::error::Error;

const MODEL_ID: &str = "minishlab/potion-base-8M";
const HEADING_WEIGHT: f32 = 0.3;

pub struct Embedder {
    model: StaticModel,
}

impl Embedder {
    pub fn new() -> Result<Self, Error> {
        let model = StaticModel::from_pretrained(MODEL_ID, None, None, None)
            .map_err(|e| Error::Embedding(e.to_string()))?;
        Ok(Embedder { model })
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let input = if text.trim().is_empty() {
            "empty"
        } else {
            text
        };
        self.model.encode_single(input)
    }

    /// Embed a chunk as a weighted average of heading and content vectors.
    pub fn embed_chunk(&self, heading: &str, content: &str) -> Vec<f32> {
        if content.trim().is_empty() {
            return self.embed_one(heading);
        }
        let h = self.embed_one(heading);
        let c = self.embed_one(content);
        weighted_average(&h, &c, HEADING_WEIGHT)
    }
}

fn weighted_average(a: &[f32], b: &[f32], a_weight: f32) -> Vec<f32> {
    let b_weight = 1.0 - a_weight;
    let mut result: Vec<f32> = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| x * a_weight + y * b_weight)
        .collect();
    // L2-normalize so distances remain comparable.
    let norm = result.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut result {
            *x /= norm;
        }
    }
    result
}
