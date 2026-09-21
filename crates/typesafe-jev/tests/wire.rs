//! The types against the documents of TypeSafe's API reference (<https://docs.typesafe.ai/api>):
//! its example requests are what the questions serialize to, byte for byte, and its example
//! responses are what the answers are read from.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use typesafe_jev::{Answer, Choice, Client, Config, Content, Error, Noul, NoulCriteria, Question, Questions, Reply, Response, Score};

/// The request body a client sends for `state` and `questions`, exactly as sent.
fn body_for<S: serde::Serialize + ?Sized>(state: &S, questions: &Questions) -> String {
    let seen = Arc::new(Mutex::new(String::new()));
    let log = Arc::clone(&seen);
    let transport = move |raw: &[u8]| {
        *log.lock().unwrap() = String::from_utf8(raw.to_vec()).unwrap();
        Ok(Reply { status: 200, retry_after: None, body: r#"{"model": "jev-test", "answers": {}}"#.into() })
    };
    Client::with_transport(transport, Config::default()).ask(state, questions).unwrap();
    let body = seen.lock().unwrap().clone();
    body
}

#[test]
fn the_reference_requests_are_what_the_questions_serialize_to() {
    const STATE: &str = "Help! My payouts have been failing for 3 days.";
    let noul = Questions::new().with("is_urgent", Noul::new("Does this convey urgency?"));
    assert_eq!(
        body_for(STATE, &noul),
        r#"{"model":"jev-latest","state":"Help! My payouts have been failing for 3 days.","questions":{"is_urgent":{"type":"noul","instructions":"Does this convey urgency?"}}}"#
    );

    let described = Questions::new()
        .with("is_urgent", Noul::new("Does this convey urgency?").yes("Explicitly time-sensitive").no("No urgency expressed"));
    assert_eq!(
        body_for(STATE, &described),
        r#"{"model":"jev-latest","state":"Help! My payouts have been failing for 3 days.","questions":{"is_urgent":{"type":"noul","instructions":"Does this convey urgency?","criteria":{"true":"Explicitly time-sensitive","false":"No urgency expressed"}}}}"#
    );

    let choice = Questions::new().with(
        "department",
        Choice::new(
            "Which team should handle this?",
            [
                ("billing", "Payments, invoicing, refunds"),
                ("technical", "Bugs, outages, integrations"),
                ("sales", "Pricing, upgrades, new accounts"),
            ],
        ),
    );
    assert_eq!(
        body_for(STATE, &choice),
        r#"{"model":"jev-latest","state":"Help! My payouts have been failing for 3 days.","questions":{"department":{"type":"choice","instructions":"Which team should handle this?","criteria":{"billing":"Payments, invoicing, refunds","technical":"Bugs, outages, integrations","sales":"Pricing, upgrades, new accounts"}}}}"#
    );

    let score = Questions::new().with("frustration", Score::new("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"]));
    assert_eq!(
        body_for(STATE, &score),
        r#"{"model":"jev-latest","state":"Help! My payouts have been failing for 3 days.","questions":{"frustration":{"type":"score","instructions":"How frustrated is the customer?","criteria":["Calm","Frustrated","Very angry"]}}}"#
    );
}

#[test]
fn questions_and_options_go_out_in_the_order_they_were_added() {
    let names = ["zeta", "alpha", "mid", "beta"];
    let questions: Questions = names.iter().map(|name| (*name, Choice::labels("Which?", names))).collect();
    let body = body_for("state", &questions);
    let positions: Vec<usize> = names.iter().map(|name| body.find(&format!("\"{name}\":{{")).unwrap()).collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{body}");
    assert!(body.contains(r#""criteria":{"zeta":null,"alpha":null,"mid":null,"beta":null}"#), "{body}");

    let mut replaced = Questions::new().with("a", Noul::new("first")).with("b", Noul::new("second"));
    let old = replaced.insert("a", Noul::new("third"));
    assert_eq!(old, Some(Question::Noul(Noul::new("first"))));
    assert_eq!(replaced.ids().collect::<Vec<_>>(), ["a", "b"], "a replaced question keeps its place");
    replaced.extend([("c", Noul::new("fourth"))]);
    assert_eq!((replaced.len(), replaced.is_empty(), replaced.get("c").is_some()), (3, false, true));
    assert_eq!(replaced.iter().count(), (&replaced).into_iter().count());
}

#[test]
fn instructions_and_criteria_take_structure_and_may_be_left_out() {
    let field = json!({"name": "ship_date", "format": "ISO 8601"});
    let structured = Content::try_from(json!({"field": field, "question": "Is `field` filled in correctly?"})).unwrap();
    let levels = [Content::try_from(json!({"label": "wrong", "examples": ["tomorrow"]})).unwrap(), Content::from("right")];
    let mut bare = Noul::new("dropped below");
    bare.instructions = None;
    let questions = Questions::new()
        .with("valid", Noul::new(structured.clone()))
        .with("quality", Score::new(structured, levels))
        .with("kind", Choice::labels(Content::try_from(json!(["Which kind?", "Pick one."])).unwrap(), ["date", "text"]))
        .with("bare", bare);
    let sent: Value = serde_json::from_str(&body_for(&json!({"ship_date": "2026-09-21"}), &questions)).unwrap();
    assert_eq!(
        sent["questions"],
        json!({
            "valid": {"type": "noul", "instructions": {"field": field, "question": "Is `field` filled in correctly?"}},
            "quality": {
                "type": "score",
                "instructions": {"field": field, "question": "Is `field` filled in correctly?"},
                "criteria": [{"label": "wrong", "examples": ["tomorrow"]}, "right"],
            },
            "kind": {"type": "choice", "instructions": ["Which kind?", "Pick one."], "criteria": {"date": null, "text": null}},
            "bare": {"type": "noul"},
        })
    );
    for not_content in [json!(null), json!(true), json!(3), json!(0.5)] {
        assert!(matches!(Content::try_from(not_content.clone()), Err(Error::InvalidRequest(_))), "{not_content}");
    }
    assert_eq!(Value::from(Content::from("text")), json!("text"));
}

#[test]
fn questions_read_back_from_the_json_they_were_written_as() {
    let questions = Questions::new()
        .with("is_urgent", Noul::new("Does this convey urgency?").yes("Explicitly time-sensitive"))
        .with("department", Choice::new("Which team?", [("billing", "Payments")]).label("other"))
        .with("frustration", Score::new("How frustrated?", ["Calm", "Very angry"]));
    let text = serde_json::to_string(&questions).unwrap();
    let back: Questions = serde_json::from_str(&text).unwrap();
    assert_eq!(back, questions);
    assert_eq!(serde_json::to_string(&back).unwrap(), text, "and are written the same again, in the same order");
    let Some(Question::Noul(noul)) = back.get("is_urgent") else { panic!("a noul") };
    let expected = serde_json::from_value::<NoulCriteria>(json!({"true": "Explicitly time-sensitive"})).unwrap();
    assert_eq!(noul.criteria.as_ref(), Some(&expected));

    for (bad, reason) in [
        (json!({"q": {"type": "ranking", "instructions": "?"}}), "unknown variant `ranking`"),
        (json!({"q": {"instructions": "?"}}), "missing field `type`"),
        (json!({"q": {"type": "score", "instructions": "?"}}), "missing field `criteria`"),
        (json!({"q": {"type": "choice", "instructions": "?", "criteria": ["a", "b"]}}), "expected a map"),
        (json!({"q": {"type": "noul", "instructions": 7}}), "a string, an object or an array"),
    ] {
        let err = serde_json::from_value::<Questions>(bad.clone()).unwrap_err().to_string();
        assert!(err.contains(reason), "{bad} -> {err}");
    }
}

#[test]
fn the_reference_responses_are_what_the_answers_are_read_from() {
    let response: Response = serde_json::from_str(
        r#"{
          "model": "jev-1.13.0",
          "answers": {
            "department": {
              "type": "choice",
              "choice": "technical",
              "confidence": 0.78,
              "probabilities": {"technical": 0.85, "sales": 0.0, "billing": 0.15}
            },
            "frustration": {
              "type": "score",
              "score": 1.05,
              "confidence": 0.92,
              "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
              "probabilities": {"0": 0.0, "1": 0.95, "2": 0.05}
            },
            "is_urgent": {"type": "noul", "noul": 0.95}
          },
          "usage": {"input_tokens": 392, "output_tokens": 65}
        }"#,
    )
    .unwrap();
    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!((response.usage.input_tokens, response.usage.output_tokens), (Some(392), Some(65)));

    let department = response.choice("department").unwrap();
    assert_eq!((department.choice.as_str(), department.confidence), ("technical", 0.78));
    assert_eq!(
        department.probabilities.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
        [("technical", 0.85), ("sales", 0.0), ("billing", 0.15)]
    );

    let frustration = response.score("frustration").unwrap();
    assert_eq!((frustration.score, frustration.confidence), (1.05, 0.92));
    assert_eq!(frustration.probabilities.iter().map(|(level, p)| (*level, *p)).collect::<Vec<_>>(), [(0, 0.0), (1, 0.95), (2, 0.05)]);
    assert_eq!(frustration.legend[&2].as_str(), Some("Very angry"));

    assert_eq!(response.noul("is_urgent").unwrap().noul, 0.95);
    assert!(response.answers["is_urgent"].as_noul().is_some() && response.answers["is_urgent"].as_score().is_none());

    // Written again, an answer is the document it was read from.
    assert_eq!(
        serde_json::to_value(&response.answers["frustration"]).unwrap(),
        json!({
            "type": "score",
            "score": 1.05,
            "confidence": 0.92,
            "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
            "probabilities": {"0": 0.0, "1": 0.95, "2": 0.05},
        })
    );
    assert_eq!(serde_json::from_value::<Response>(serde_json::to_value(&response).unwrap()).unwrap(), response);
}

#[test]
fn score_levels_are_numbers_in_order_and_a_structured_level_comes_back_structured() {
    let answer: Answer = serde_json::from_value(json!({
        "type": "score",
        "score": 9.5,
        "confidence": 0.6,
        "legend": {"10": "best", "9": {"label": "nearly", "examples": ["one typo"]}, "2": "poor"},
        "probabilities": {"10": 0.5, "9": 0.5, "2": 0.0},
    }))
    .unwrap();
    let score = answer.as_score().unwrap();
    assert_eq!(
        score.probabilities.keys().copied().collect::<Vec<_>>(),
        [2, 9, 10],
        "by number, not as text, where \"10\" sorts before \"2\""
    );
    assert!(matches!(&score.legend[&9], Content::Object(level) if level["label"] == "nearly"));
    assert_eq!(score.legend[&10].as_str(), Some("best"));
}

#[test]
fn answers_can_be_built_for_fakes() {
    use typesafe_jev::{ChoiceAnswer, NoulAnswer, ScoreAnswer};
    let noul = Answer::Noul(NoulAnswer::new(0.5));
    assert_eq!(serde_json::to_value(&noul).unwrap(), json!({"type": "noul", "noul": 0.5}));
    let choice = Answer::Choice(ChoiceAnswer::new("a", 1.0, [("a".to_owned(), 1.0)].into_iter().collect()));
    assert_eq!(
        serde_json::to_value(&choice).unwrap(),
        json!({"type": "choice", "choice": "a", "confidence": 1.0, "probabilities": {"a": 1.0}})
    );
    let score = Answer::Score(ScoreAnswer::new(0.0, 1.0, [(0, Content::from("low"))].into(), [(0, 1.0)].into()));
    assert_eq!(serde_json::from_value::<Answer>(serde_json::to_value(&score).unwrap()).unwrap(), score);
}
