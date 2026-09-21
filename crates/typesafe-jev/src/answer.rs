use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::Content;

/// What one request came back with: an [`Answer`] per question, under the id the question had.
///
/// ```
/// use typesafe_jev::{Answer, Response};
///
/// let response: Response = serde_json::from_str(r#"{
///     "model": "jev-1.13.0",
///     "answers": {
///         "is_urgent": {"type": "noul", "noul": 0.95},
///         "department": {"type": "choice", "choice": "billing", "confidence": 0.81,
///                        "probabilities": {"billing": 0.88, "technical": 0.12, "sales": 0.0}}
///     },
///     "usage": {"input_tokens": 318, "output_tokens": 34}
/// }"#)?;
///
/// assert_eq!(response.noul("is_urgent").map(|answer| answer.noul), Some(0.95));
/// let department = response.choice("department").expect("asked as a choice");
/// assert_eq!((department.choice.as_str(), department.probabilities["technical"]), ("billing", 0.12));
/// assert!(response.score("department").is_none(), "it is there, but it is not a score");
/// assert!(matches!(response.answers["is_urgent"], Answer::Noul(_)));
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Response {
    /// The model that performed the evaluation: the release that an alias like `jev-latest`
    /// stood for.
    pub model: String,
    /// One answer per question, keyed and ordered as the API returned them.
    pub answers: IndexMap<String, Answer>,
    /// The tokens this request used.
    #[serde(default)]
    pub usage: TokenUsage,
}

impl Response {
    /// The answer under `id`, if there is one and it answers a [`Noul`](crate::Noul).
    pub fn noul(&self, id: &str) -> Option<&NoulAnswer> {
        self.answers.get(id).and_then(Answer::as_noul)
    }

    /// The answer under `id`, if there is one and it answers a [`Choice`](crate::Choice).
    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        self.answers.get(id).and_then(Answer::as_choice)
    }

    /// The answer under `id`, if there is one and it answers a [`Score`](crate::Score).
    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        self.answers.get(id).and_then(Answer::as_score)
    }
}

/// The tokens one request used, as the API reports them. A count the API left out is `None`.
///
/// [`Usage`](crate::Usage) is the running total a [`Client`](crate::Client) keeps of these.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TokenUsage {
    /// Tokens of state and questions. These are what Jev charges for.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Tokens of answers. Jev reports them, but does not charge for them.
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

/// The answer to one [`Question`](crate::Question), of the type the question had.
///
/// A [`ChoiceAnswer`] and a [`ScoreAnswer`] carry a `confidence` next to their probabilities: the
/// probabilities say which answer, the confidence says how far to trust it. A [`NoulAnswer`] is
/// a single calibrated probability and needs none.
///
/// The enum is `#[non_exhaustive]`, like [`Question`](crate::Question).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", try_from = "RawAnswer")]
#[non_exhaustive]
pub enum Answer {
    /// The answer to a [`Noul`](crate::Noul).
    Noul(NoulAnswer),
    /// The answer to a [`Choice`](crate::Choice).
    Choice(ChoiceAnswer),
    /// The answer to a [`Score`](crate::Score).
    Score(ScoreAnswer),
}

impl Answer {
    /// The answer, if it answers a [`Noul`](crate::Noul).
    pub fn as_noul(&self) -> Option<&NoulAnswer> {
        match self {
            Answer::Noul(answer) => Some(answer),
            _ => None,
        }
    }

    /// The answer, if it answers a [`Choice`](crate::Choice).
    pub fn as_choice(&self) -> Option<&ChoiceAnswer> {
        match self {
            Answer::Choice(answer) => Some(answer),
            _ => None,
        }
    }

    /// The answer, if it answers a [`Score`](crate::Score).
    pub fn as_score(&self) -> Option<&ScoreAnswer> {
        match self {
            Answer::Score(answer) => Some(answer),
            _ => None,
        }
    }
}

/// The answer to a yes/no question.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub struct NoulAnswer {
    /// The probability that the answer is yes, from 0 (no) to 1 (yes). It is calibrated: of the
    /// answers given as 0.9, about nine in ten are a yes. Near 0.5 the model does not know.
    pub noul: f64,
}

impl NoulAnswer {
    /// An answer with this probability of a yes, for tests and fakes.
    pub fn new(noul: f64) -> Self {
        NoulAnswer { noul }
    }
}

/// The answer to a [`Choice`](crate::Choice): the selected option and the probability of each.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ChoiceAnswer {
    /// The option with the highest probability.
    pub choice: String,
    /// How certain the model is of the selection, from 0 to 1, derived from `probabilities`.
    pub confidence: f64,
    /// Every option of the question and its probability. They sum to about 1.
    pub probabilities: IndexMap<String, f64>,
}

impl ChoiceAnswer {
    /// An answer, for tests and fakes.
    pub fn new(choice: impl Into<String>, confidence: f64, probabilities: IndexMap<String, f64>) -> Self {
        ChoiceAnswer { choice: choice.into(), confidence, probabilities }
    }
}

/// The answer to a [`Score`](crate::Score): a position on the question's scale.
///
/// Levels are numbered from 0 in the order of the question's `criteria`. On the wire the keys of
/// `legend` and `probabilities` are those numbers as strings; here they are numbers, in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ScoreAnswer {
    /// The probability-weighted average of the levels. It can fall between two levels.
    pub score: f64,
    /// How certain the model is of the score, from 0 to 1, derived from `probabilities`.
    pub confidence: f64,
    /// Each level and the description the question gave it.
    pub legend: BTreeMap<u32, Content>,
    /// Each level and its probability. They sum to about 1.
    pub probabilities: BTreeMap<u32, f64>,
}

impl ScoreAnswer {
    /// An answer, for tests and fakes.
    pub fn new(score: f64, confidence: f64, legend: BTreeMap<u32, Content>, probabilities: BTreeMap<u32, f64>) -> Self {
        ScoreAnswer { score, confidence, legend, probabilities }
    }
}

/// An answer as it is on the wire, before its `type` says which fields it must have.
///
/// `Answer` is read through this rather than through serde's own internally tagged enums: those
/// buffer the object to find the tag, which garbles `f64` fields when a dependent crate turns
/// on `serde_json`'s `arbitrary_precision`, and their errors do not say which field is missing
/// from which type of answer.
#[derive(Deserialize)]
struct RawAnswer {
    r#type: String,
    noul: Option<f64>,
    choice: Option<String>,
    score: Option<f64>,
    confidence: Option<f64>,
    probabilities: Option<IndexMap<String, f64>>,
    legend: Option<IndexMap<String, Content>>,
}

impl TryFrom<RawAnswer> for Answer {
    type Error = String;

    fn try_from(raw: RawAnswer) -> Result<Self, String> {
        fn required<T>(field: Option<T>, name: &str, kind: &str) -> Result<T, String> {
            field.ok_or_else(|| format!("a `{kind}` answer has no `{name}`"))
        }
        fn by_level<T>(map: IndexMap<String, T>, name: &str) -> Result<BTreeMap<u32, T>, String> {
            map.into_iter()
                .map(|(level, value)| match level.parse() {
                    Ok(level) => Ok((level, value)),
                    Err(_) => Err(format!("a `score` answer's `{name}` has the key `{level}`, which is not a level number")),
                })
                .collect()
        }
        let kind = raw.r#type.as_str();
        match kind {
            "noul" => Ok(Answer::Noul(NoulAnswer { noul: required(raw.noul, "noul", kind)? })),
            "choice" => Ok(Answer::Choice(ChoiceAnswer {
                choice: required(raw.choice, "choice", kind)?,
                confidence: required(raw.confidence, "confidence", kind)?,
                probabilities: required(raw.probabilities, "probabilities", kind)?,
            })),
            "score" => Ok(Answer::Score(ScoreAnswer {
                score: required(raw.score, "score", kind)?,
                confidence: required(raw.confidence, "confidence", kind)?,
                legend: by_level(required(raw.legend, "legend", kind)?, "legend")?,
                probabilities: by_level(required(raw.probabilities, "probabilities", kind)?, "probabilities")?,
            })),
            other => Err(format!("unknown answer type `{other}`; this client reads `noul`, `choice` and `score`")),
        }
    }
}
