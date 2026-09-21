//! Asks Jev two questions about a snippet of code, then prints the answers and what they cost.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example ask
//! ```
//!
//! This sends the snippet below to TypeSafe's API and is billed to the key.

use serde_json::{json, Map};
use typesafe_jev::{Client, Config, Error};

fn main() -> Result<(), Error> {
    let client = Client::from_env(Config::default())?;

    let state = json!({
        "code": "1| import os\n2| def cwd():\n3|     return os.getcwd()",
        "query": "where is the working directory read",
    });
    let mut questions = Map::new();
    questions.insert(
        "line3".into(),
        json!({
            "type": "noul",
            "instructions": "Does line 3 of the code directly answer the query?",
        }),
    );
    questions.insert(
        "relevance".into(),
        json!({
            "type": "score",
            "instructions": "How relevant is the code to the query?",
            "criteria": ["unrelated", "tangential", "relevant", "exactly what was asked"],
        }),
    );

    match client.ask(&state, &questions) {
        Ok(answers) => {
            for (id, answer) in &answers {
                println!("{id}: {answer}");
            }
        }
        // Too much state for one request: a real caller would split it and ask again.
        Err(Error::TokenLimit(message)) => eprintln!("request too large: {message}"),
        Err(other) => return Err(other),
    }

    let usage = client.usage();
    println!(
        "{} request(s), {} retries, {} input tokens, ~${:.6}",
        usage.requests(),
        usage.retries(),
        usage.input_tokens(),
        usage.cost_usd()
    );
    Ok(())
}
