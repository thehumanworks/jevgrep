//! Asks Jev three questions about a support ticket, one of each type, then prints the answers
//! and what they cost.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example ask
//! ```
//!
//! This sends the ticket below to TypeSafe's API and is billed to the key.

use serde::Serialize;
use typesafe_jev::{Choice, Client, Config, Error, Noul, Questions, Score};

/// The state can be any type that serializes to a JSON object, array or string.
#[derive(Serialize)]
struct Ticket<'a> {
    customer_plan: &'a str,
    message: &'a str,
}

fn main() -> Result<(), Error> {
    let client = Client::from_env(Config::default())?;

    let ticket = Ticket {
        customer_plan: "business",
        message: "Hi, I've been trying to connect my Stripe account for 3 days and the integration keeps failing. \
                  I'm losing sales. Please help ASAP.",
    };
    let questions = Questions::new()
        .with(
            "department",
            Choice::new(
                "Which team should handle the `message`?",
                [
                    ("billing", "Payment or subscription issues"),
                    ("technical", "Bugs or integration problems"),
                    ("sales", "Pricing or account questions"),
                ],
            ),
        )
        .with(
            "frustration",
            Score::new(
                "How frustrated the customer appears",
                ["Calm, just stating facts", "Frustrated but civil", "Very angry, strong language"],
            ),
        )
        .with("is_urgent", Noul::new("The message conveys urgency or time-sensitivity"));

    let response = match client.ask(&ticket, &questions) {
        Ok(response) => response,
        // Too much state for one request: a real caller would split it and ask again.
        Err(Error::TokenLimit(message)) => {
            eprintln!("request too large: {message}");
            return Ok(());
        }
        Err(other) => return Err(other),
    };

    if let Some(department) = response.choice("department") {
        println!("department: {} (confidence {:.2})", department.choice, department.confidence);
        for (option, probability) in &department.probabilities {
            println!("  {option}: {probability:.2}");
        }
    }
    if let Some(frustration) = response.score("frustration") {
        println!("frustration: {:.2} of {} (confidence {:.2})", frustration.score, frustration.legend.len() - 1, frustration.confidence);
    }
    if let Some(urgent) = response.noul("is_urgent") {
        println!("urgent: p={:.2}", urgent.noul);
    }

    let usage = client.usage();
    println!(
        "{}: {} request(s), {} retries, {} input tokens, ~${:.6}",
        response.model,
        usage.requests(),
        usage.retries(),
        usage.input_tokens(),
        usage.cost_usd()
    );
    Ok(())
}
