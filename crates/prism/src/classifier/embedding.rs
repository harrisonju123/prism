use std::sync::LazyLock;

use crate::cache::semantic::{cosine_similarity, simple_text_embedding};
use crate::classifier::taxonomy::{ClassificationResult, ClassifierInput, TASK_KEYWORDS};
use crate::types::TaskType;

const DEFAULT_DIM: usize = 128;

pub struct EmbeddingClassifier {
    prototypes: Vec<(TaskType, Vec<f32>)>,
    dim: usize,
}

static INSTANCE: LazyLock<EmbeddingClassifier> =
    LazyLock::new(|| EmbeddingClassifier::build(DEFAULT_DIM));

impl EmbeddingClassifier {
    /// Return reference to the shared static instance.
    pub fn get() -> &'static Self {
        &INSTANCE
    }

    /// Build an instance with a specific dim (used for config-driven override or testing).
    pub fn build(dim: usize) -> Self {
        let prototypes = TASK_KEYWORDS
            .iter()
            .filter_map(|(task_type, keywords)| {
                if keywords.is_empty() {
                    return None;
                }
                let sum: Vec<f32> = keywords
                    .iter()
                    .map(|kw| simple_text_embedding(kw, dim))
                    .fold(vec![0.0_f32; dim], |mut acc, emb| {
                        for (a, e) in acc.iter_mut().zip(emb.iter()) {
                            *a += e;
                        }
                        acc
                    });
                let n = keywords.len() as f32;
                let mut avg: Vec<f32> = sum.iter().map(|v| v / n).collect();
                let norm: f32 = avg.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > f32::EPSILON {
                    for v in &mut avg {
                        *v /= norm;
                    }
                    Some((*task_type, avg))
                } else {
                    None
                }
            })
            .collect();
        Self { prototypes, dim }
    }

    pub fn classify(&self, input: &ClassifierInput) -> ClassificationResult {
        let query = match &input.system_prompt_text {
            Some(sys) if !sys.is_empty() => {
                let prefix: String = sys.chars().take(200).collect();
                format!("{} {}", prefix, input.last_user_message)
            }
            _ => input.last_user_message.clone(),
        };

        if query.trim().is_empty() {
            return ClassificationResult {
                task_type: TaskType::Unknown,
                confidence: 0.0,
                signals: vec![],
            };
        }

        let query_emb = simple_text_embedding(&query, self.dim);

        let best = self
            .prototypes
            .iter()
            .map(|(task_type, proto)| {
                let sim = cosine_similarity(&query_emb, proto);
                (*task_type, sim)
            })
            .max_by(|(_, a), (_, b)| a.total_cmp(b));

        match best {
            Some((task_type, similarity)) => {
                let confidence = (similarity as f64 * 1.5).min(1.0);
                let signals = vec![format!("embedding_similarity={:.3}", similarity)];
                ClassificationResult {
                    task_type,
                    confidence,
                    signals,
                }
            }
            None => ClassificationResult {
                task_type: TaskType::Unknown,
                confidence: 0.0,
                signals: vec![],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prototypes_built_for_all_task_keywords() {
        let clf = EmbeddingClassifier::build(64);
        assert!(!clf.prototypes.is_empty());
    }

    #[test]
    fn test_classify_summarization_semantic() {
        let clf = EmbeddingClassifier::build(128);
        let input = ClassifierInput {
            last_user_message: "give me an overview of the key points in this document".to_string(),
            ..Default::default()
        };
        let result = clf.classify(&input);
        assert_eq!(
            result.task_type,
            TaskType::Summarization,
            "embedding should match summarization-flavored text"
        );
    }

    #[test]
    fn test_classify_code_generation() {
        let clf = EmbeddingClassifier::build(128);
        let input = ClassifierInput {
            last_user_message: "write a function that reverses a string".to_string(),
            ..Default::default()
        };
        let result = clf.classify(&input);
        assert_eq!(result.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn test_classify_empty_input_returns_unknown() {
        let clf = EmbeddingClassifier::build(128);
        let input = ClassifierInput {
            last_user_message: "".to_string(),
            ..Default::default()
        };
        let result = clf.classify(&input);
        assert_eq!(result.task_type, TaskType::Unknown);
    }
}
