use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::Content;

/// One typed question: a [`Noul`], a [`Choice`] or a [`Score`], told apart on the wire by its
/// `type` field.
///
/// Each of the three converts with `into()`, which [`Questions::with`] and
/// [`Questions::insert`] do for you. The enum is `#[non_exhaustive]`: System One may gain
/// question types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Question {
    /// A yes/no question, answered by a [`NoulAnswer`](crate::NoulAnswer).
    Noul(Noul),
    /// One option out of a set, answered by a [`ChoiceAnswer`](crate::ChoiceAnswer).
    Choice(Choice),
    /// A rating on an ordered scale, answered by a [`ScoreAnswer`](crate::ScoreAnswer).
    Score(Score),
}

impl From<Noul> for Question {
    fn from(question: Noul) -> Self {
        Question::Noul(question)
    }
}

impl From<Choice> for Question {
    fn from(question: Choice) -> Self {
        Question::Choice(question)
    }
}

impl From<Score> for Question {
    fn from(question: Score) -> Self {
        Question::Score(question)
    }
}

/// A yes/no question. The answer is the probability that the answer is yes.
///
/// ```
/// use typesafe_jev::{Noul, Question};
///
/// let question = Noul::new("Does this convey urgency?")
///     .yes("Explicitly time-sensitive")
///     .no("No urgency expressed");
/// assert_eq!(
///     serde_json::to_value(Question::from(question))?,
///     serde_json::json!({
///         "type": "noul",
///         "instructions": "Does this convey urgency?",
///         "criteria": {"true": "Explicitly time-sensitive", "false": "No urgency expressed"},
///     }),
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Noul {
    /// The yes/no question to evaluate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// What a yes and a no mean, when the question alone does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

impl Noul {
    /// A yes/no question with no description of either outcome.
    pub fn new(instructions: impl Into<Content>) -> Self {
        Noul { instructions: Some(instructions.into()), criteria: None }
    }

    /// Describes what a yes (an answer near 1) means.
    #[must_use]
    pub fn yes(mut self, meaning: impl Into<Content>) -> Self {
        self.criteria.get_or_insert_with(NoulCriteria::default).yes = Some(meaning.into());
        self
    }

    /// Describes what a no (an answer near 0) means.
    #[must_use]
    pub fn no(mut self, meaning: impl Into<Content>) -> Self {
        self.criteria.get_or_insert_with(NoulCriteria::default).no = Some(meaning.into());
        self
    }
}

/// What the two outcomes of a [`Noul`] mean. On the wire the fields are `true` and `false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoulCriteria {
    /// What a yes (an answer near 1) means.
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<Content>,
    /// What a no (an answer near 0) means.
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub no: Option<Content>,
}

/// Picks one option from a set you define. The answer names the most probable option and gives
/// the probability of each.
///
/// The options keep the order they are given in. The API accepts up to 255 of them.
///
/// ```
/// use typesafe_jev::Choice;
///
/// let department = Choice::new("Which team should handle this?", [
///     ("billing", "Payments, invoicing, refunds"),
///     ("technical", "Bugs, outages, integrations"),
/// ])
/// .option("sales", "Pricing, upgrades, new accounts");
/// assert_eq!(department.criteria.keys().collect::<Vec<_>>(), ["billing", "technical", "sales"]);
///
/// // Options whose names say it all need no description.
/// let tone = Choice::labels("What is the tone?", ["calm", "frustrated", "angry"]);
/// assert_eq!(tone.criteria["calm"], None);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// What the model should decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// Each option and its description, in order. `None` (JSON `null`) is an option that needs
    /// no more detail than its name.
    pub criteria: IndexMap<String, Option<Content>>,
}

impl Choice {
    /// A choice between described options.
    pub fn new<N, D>(instructions: impl Into<Content>, options: impl IntoIterator<Item = (N, D)>) -> Self
    where
        N: Into<String>,
        D: Into<Content>,
    {
        let criteria = options.into_iter().map(|(name, description)| (name.into(), Some(description.into()))).collect();
        Choice { instructions: Some(instructions.into()), criteria }
    }

    /// A choice between options that their names describe well enough.
    pub fn labels<N: Into<String>>(instructions: impl Into<Content>, names: impl IntoIterator<Item = N>) -> Self {
        let criteria = names.into_iter().map(|name| (name.into(), None)).collect();
        Choice { instructions: Some(instructions.into()), criteria }
    }

    /// Adds a described option, or replaces the description of one that is there.
    #[must_use]
    pub fn option(mut self, name: impl Into<String>, description: impl Into<Content>) -> Self {
        self.criteria.insert(name.into(), Some(description.into()));
        self
    }

    /// Adds an option with no description.
    #[must_use]
    pub fn label(mut self, name: impl Into<String>) -> Self {
        self.criteria.insert(name.into(), None);
        self
    }
}

/// Rates the state against ordered, described levels. The answer is a probability-weighted
/// position on that scale, which can fall between two levels.
///
/// A level's number is its position in `criteria`, from 0, from the low end of the scale to the
/// high end. A score should have at least two levels; the API accepts up to 10.
///
/// ```
/// use typesafe_jev::Score;
///
/// let frustration = Score::new("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"]);
/// assert_eq!(frustration.criteria.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Score {
    /// What the model should rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// The description of each level, from level 0 up.
    pub criteria: Vec<Content>,
}

impl Score {
    /// A rating against `levels`, given from the low end of the scale to the high end.
    pub fn new<L: Into<Content>>(instructions: impl Into<Content>, levels: impl IntoIterator<Item = L>) -> Self {
        Score { instructions: Some(instructions.into()), criteria: levels.into_iter().map(Into::into).collect() }
    }
}

/// The questions of one request, each under an id you choose. The answer comes back under the
/// same id; the id itself is not shown to the model.
///
/// Questions keep the order they are added in, and every question of a request is evaluated
/// against the same state independently: one answer is never context for another.
///
/// ```
/// use typesafe_jev::{Choice, Noul, Questions, Score};
///
/// let questions = Questions::new()
///     .with("department", Choice::labels("Which team should handle this?", ["billing", "technical", "sales"]))
///     .with("frustration", Score::new("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"]))
///     .with("is_urgent", Noul::new("Does this convey urgency?"));
/// assert_eq!(questions.len(), 3);
/// assert_eq!(questions.ids().collect::<Vec<_>>(), ["department", "frustration", "is_urgent"]);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Questions(IndexMap<String, Question>);

impl Questions {
    /// No questions yet.
    pub fn new() -> Self {
        Questions::default()
    }

    /// Adds a question and returns the set, for chaining.
    #[must_use]
    pub fn with(mut self, id: impl Into<String>, question: impl Into<Question>) -> Self {
        self.insert(id, question);
        self
    }

    /// Adds a question. A question already under `id` is replaced, in place, and returned.
    pub fn insert(&mut self, id: impl Into<String>, question: impl Into<Question>) -> Option<Question> {
        self.0.insert(id.into(), question.into())
    }

    /// The question under `id`.
    pub fn get(&self, id: &str) -> Option<&Question> {
        self.0.get(id)
    }

    /// How many questions there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no questions.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The ids, in order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The ids and their questions, in order.
    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Question> {
        self.0.iter()
    }

    /// The questions as the map they are.
    pub fn as_map(&self) -> &IndexMap<String, Question> {
        &self.0
    }
}

impl From<IndexMap<String, Question>> for Questions {
    fn from(questions: IndexMap<String, Question>) -> Self {
        Questions(questions)
    }
}

impl From<Questions> for IndexMap<String, Question> {
    fn from(questions: Questions) -> Self {
        questions.0
    }
}

impl<I: Into<String>, Q: Into<Question>> FromIterator<(I, Q)> for Questions {
    fn from_iter<T: IntoIterator<Item = (I, Q)>>(questions: T) -> Self {
        Questions(questions.into_iter().map(|(id, question)| (id.into(), question.into())).collect())
    }
}

impl<I: Into<String>, Q: Into<Question>> Extend<(I, Q)> for Questions {
    fn extend<T: IntoIterator<Item = (I, Q)>>(&mut self, questions: T) {
        self.0.extend(questions.into_iter().map(|(id, question)| (id.into(), question.into())));
    }
}

impl IntoIterator for Questions {
    type Item = (String, Question);
    type IntoIter = indexmap::map::IntoIter<String, Question>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Questions {
    type Item = (&'a String, &'a Question);
    type IntoIter = indexmap::map::Iter<'a, String, Question>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
