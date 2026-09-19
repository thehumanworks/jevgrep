//! Jev-shaped answers from a text model: the terse wire format, its schema, and its validation.
//!
//! Shared by every backend that asks a generative model (ChatGPT, OpenRouter). Questions travel
//! under positional ids and are answered with bare percentages; `decode_answers` checks the reply
//! against the questions that were asked and rebuilds the shape Jev returns. A backend whose
//! endpoint can enforce a schema sends `request_schema`; one that cannot still validates the same way.

use serde_json::{json, Map, Value};

use crate::client::JevError;

/// OpenAI's structured outputs top out here, and the ChatGPT endpoint with them. Hitting a limit is
/// a `TokenLimit`, so callers that can split their questions (search.rs splits the chunk) do so
/// instead of failing.
const MAX_SCHEMA_PROPERTIES: usize = 5000;
const MAX_SCHEMA_STRING_CHARS: usize = 120_000;
/// A probability list has to sum to one, give or take the model's rounding to whole percentages.
const PROBABILITY_SUM_TOLERANCE: f64 = 0.025;

// ---------------------------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------------------------

/// A strict-mode object: every declared property is required and nothing else may appear.
pub(crate) fn strict_object(properties: Map<String, Value>) -> Value {
    let required: Vec<Value> = properties.keys().map(|k| json!(k)).collect();
    json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties})
}

pub(crate) fn props(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// `criteria` as a list of level descriptions, or a clear error.
fn criteria_of(qid: &str, question: &Value) -> Result<Vec<String>, JevError> {
    let listed = question
        .get("criteria")
        .and_then(Value::as_array)
        .ok_or_else(|| JevError::Api(format!("question `{qid}` is type `score` but has no `criteria` array")))?;
    let criteria: Option<Vec<String>> = listed.iter().map(|c| c.as_str().map(str::to_owned)).collect();
    match criteria {
        Some(criteria) if !criteria.is_empty() => Ok(criteria),
        Some(_) => Err(JevError::Api(format!("question `{qid}` has an empty `criteria` array"))),
        None => Err(JevError::Api(format!("question `{qid}` has non-string entries in `criteria`"))),
    }
}

/// An integer percentage. The bounds are declared as well as checked: a live probe confirmed this
/// endpoint honours `minimum`/`maximum` under strict mode, and the range checks in `decode_answer`
/// still stand behind them.
fn percentage() -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": 100})
}

/// The schema for one answer, in the terse wire shape `INSTRUCTIONS` describes.
fn answer_schema(qid: &str, question: &Value) -> Result<Value, JevError> {
    match question.get("type").and_then(Value::as_str) {
        Some("noul") => Ok(percentage()),
        Some("score") => {
            let levels = criteria_of(qid, question)?.len();
            let probabilities = json!({"type": "array", "items": percentage(), "minItems": levels, "maxItems": levels});
            Ok(strict_object(props([("confidence", percentage()), ("probabilities", probabilities)])))
        }
        Some(other) => {
            Err(JevError::Api(format!("question `{qid}` has unsupported type `{other}`; this backend answers `noul` and `score`")))
        }
        None => Err(JevError::Api(format!("question `{qid}` has no `type`"))),
    }
}

/// The id a question travels under: its position in the caller's map. Every answer repeats its id,
/// so `q0.L117` on the wire would be paid for in output tokens once per line of source.
pub(crate) fn wire_id(index: usize) -> String {
    index.to_string()
}

/// The whole response schema: `{"answers": {<wire id>: <answer>, ...}}`.
pub(crate) fn request_schema(questions: &Map<String, Value>) -> Result<Value, JevError> {
    let mut answers = Map::new();
    for (index, (qid, question)) in questions.iter().enumerate() {
        answers.insert(wire_id(index), answer_schema(qid, question)?);
    }
    Ok(strict_object(props([("answers", strict_object(answers))])))
}

/// `request_schema`, refused when it is larger than a strict-mode endpoint accepts.
pub(crate) fn budgeted_schema(questions: &Map<String, Value>) -> Result<Value, JevError> {
    let schema = request_schema(questions)?;
    let (declared, chars) = schema_budget(&schema);
    if declared > MAX_SCHEMA_PROPERTIES {
        return Err(JevError::TokenLimit(format!(
            "{} questions need {declared} schema properties, over the {MAX_SCHEMA_PROPERTIES} limit; ask fewer questions per request",
            questions.len()
        )));
    }
    if chars > MAX_SCHEMA_STRING_CHARS {
        return Err(JevError::TokenLimit(format!(
            "response schema needs {chars} string characters, over the {MAX_SCHEMA_STRING_CHARS} limit; ask fewer questions per request"
        )));
    }
    Ok(schema)
}

/// Rejects a question map this wire format cannot carry, before anything is sent. For backends
/// that send no schema; `request_schema` makes the same checks.
pub(crate) fn check_questions(questions: &Map<String, Value>) -> Result<(), JevError> {
    questions.iter().try_for_each(|(qid, question)| answer_schema(qid, question).map(drop))
}

/// Every property the schema declares, at any depth, and the characters their names take. These
/// are what the model's schema limits count; the terse schema declares no enums.
pub(crate) fn schema_budget(schema: &Value) -> (usize, usize) {
    let mut budget = (0, 0);
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        budget.0 += properties.len();
        for (name, child) in properties {
            let (properties, chars) = schema_budget(child);
            budget.0 += properties;
            budget.1 += name.chars().count() + chars;
        }
    }
    budget
}

// ---------------------------------------------------------------------------------------------
// Answer validation
// ---------------------------------------------------------------------------------------------

/// A percentage from the wire as a probability, if it is one.
fn probability(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite() && (0.0..=100.0).contains(n)).map(|n| n / 100.0)
}

/// Lists ids in an error without letting a large request turn into a wall of text.
fn sample(ids: &[&String]) -> String {
    let shown = ids.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    if ids.len() > 5 {
        format!("{shown}, and {} more", ids.len() - 5)
    } else {
        shown
    }
}

/// One wire answer, checked and rebuilt into the shape Jev returns.
///
/// Every diagnostic here is built from the question the caller asked, never from what came back.
/// A wrong answer can contain the user's own source, or anything else the model chose to emit.
fn decode_answer(qid: &str, question: &Value, answer: &Value) -> Result<Value, JevError> {
    let wrong = |what: String| Err(JevError::Api(format!("answer for `{qid}` {what}")));
    match question.get("type").and_then(Value::as_str).unwrap_or_default() {
        "noul" => match probability(Some(answer)) {
            Some(p) => return Ok(json!({"type": "noul", "noul": p})),
            None => return wrong("is not a percentage in 0..=100".to_owned()),
        },
        "score" => {}
        // `answer_schema` refused anything else before the request went out.
        other => return wrong(format!("has the unsupported type `{other}`")),
    }

    let criteria = criteria_of(qid, question)?;
    // Strict mode forbids extra properties, so an answer carrying any is not schema-conformant.
    if answer.as_object().is_none_or(|object| object.len() != 2) {
        return wrong("does not have exactly the fields `confidence` and `probabilities`".to_owned());
    }
    let Some(confidence) = probability(answer.get("confidence")) else {
        return wrong("has no `confidence` percentage in 0..=100".to_owned());
    };
    let listed = answer.get("probabilities").and_then(Value::as_array).filter(|listed| listed.len() == criteria.len());
    let Some(listed) = listed else {
        return wrong(format!("has no `probabilities` array of exactly {} percentages", criteria.len()));
    };
    let Some(levels) = listed.iter().map(|p| probability(Some(p))).collect::<Option<Vec<f64>>>() else {
        return wrong("has a `probabilities` value outside 0..=100".to_owned());
    };
    let total: f64 = levels.iter().sum();
    if (total - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return wrong(format!("has `probabilities` summing to {:.0} rather than 100", total * 100.0));
    }
    // `score` is the mean the distribution implies. Computing it here leaves the model no way to
    // report a score that contradicts its own probabilities.
    let score = levels.iter().enumerate().map(|(index, p)| p * index as f64).sum::<f64>() / total;
    let indexed = |values: &mut dyn Iterator<Item = Value>| -> Map<String, Value> {
        values.enumerate().map(|(index, value)| (index.to_string(), value)).collect()
    };
    Ok(json!({
        "type": "score",
        "score": score,
        "confidence": confidence,
        "probabilities": indexed(&mut levels.iter().map(|p| json!(p))),
        "legend": indexed(&mut criteria.iter().map(|text| json!(text))),
    }))
}

/// Accepts the model's `answers` object only if it matches the questions exactly, and hands it
/// back keyed by the caller's ids.
///
/// Unanswered ids are named, because they come from the caller's own question map. Unexpected ids
/// are only counted: those strings came from the model.
pub(crate) fn decode_answers(questions: &Map<String, Value>, answers: &Map<String, Value>) -> Result<Map<String, Value>, JevError> {
    let missing: Vec<&String> =
        questions.keys().enumerate().filter(|(index, _)| !answers.contains_key(&wire_id(*index))).map(|(_, qid)| qid).collect();
    if !missing.is_empty() {
        return Err(JevError::Api(format!(
            "model left {} of {} questions unanswered: {}",
            missing.len(),
            questions.len(),
            sample(&missing)
        )));
    }
    // Every wire id is present, so anything beyond that count was not asked.
    let extra = answers.len() - questions.len();
    if extra > 0 {
        return Err(JevError::Api(format!("model returned {extra} answer(s) to questions that were not asked")));
    }
    questions
        .iter()
        .enumerate()
        .map(|(index, (qid, question))| Ok((qid.clone(), decode_answer(qid, question, &answers[&wire_id(index)])?)))
        .collect()
}
