//! Selection state shared by question controls; only presented question IDs are sent.
use crate::{UserInputAnswer, UserInputQuestion};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct InputAnswers(BTreeMap<String, Vec<String>>);
impl InputAnswers {
    pub fn selected(&self, id: &str) -> &[String] {
        self.0.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn choose(&mut self, question: &UserInputQuestion, label: &str) {
        if !question.options.iter().any(|option| option == label) {
            return;
        }
        let selected = self.0.entry(question.id.clone()).or_default();
        if !question.multi_select {
            *selected = vec![label.into()];
        } else if let Some(index) = selected.iter().position(|value| value == label) {
            selected.remove(index);
        } else {
            selected.push(label.into());
        }
    }
    pub fn set_text(&mut self, id: &str, text: &str) {
        let text = text.trim();
        self.0.insert(
            id.into(),
            if text.is_empty() {
                vec![]
            } else {
                vec![text.into()]
            },
        );
    }
    pub fn complete(&self, questions: &[UserInputQuestion]) -> Option<Vec<UserInputAnswer>> {
        let mut ids = BTreeSet::new();
        if questions.is_empty() {
            return None;
        }
        questions
            .iter()
            .map(|question| {
                let labels = self.selected(&question.id);
                if labels.is_empty() || !ids.insert(&question.id) {
                    return None;
                }
                Some(UserInputAnswer {
                    question_id: question.id.clone(),
                    labels: labels.to_vec(),
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn question(id: &str, multi: bool) -> UserInputQuestion {
        UserInputQuestion {
            id: id.into(),
            header: String::new(),
            question: String::new(),
            options: vec!["Allow once".into(), "Deny".into()],
            multi_select: multi,
            prefill: None,
            multiline: false,
        }
    }
    #[test]
    fn choices_replace_single_values_and_toggle_multiple_values() {
        let mut answers = InputAnswers::default();
        let single = question("permission", false);
        let multi = question("details", true);
        answers.choose(&single, "Allow once");
        answers.choose(&single, "Deny");
        answers.choose(&single, "unlisted");
        assert_eq!(answers.selected("permission"), ["Deny"]);
        answers.choose(&multi, "Allow once");
        answers.choose(&multi, "Deny");
        answers.choose(&multi, "Allow once");
        assert_eq!(answers.selected("details"), ["Deny"]);
        assert_eq!(
            answers
                .complete(&[multi, single])
                .unwrap()
                .iter()
                .map(|answer| answer.question_id.as_str())
                .collect::<Vec<_>>(),
            ["details", "permission"]
        );
    }
    #[test]
    fn free_text_is_explicit_and_incomplete_or_duplicate_questions_cannot_submit() {
        let mut answers = InputAnswers::default();
        let q = question("answer", false);
        answers.set_text("answer", "  ");
        assert!(answers.complete(&[q.clone()]).is_none());
        answers.set_text("unrelated", "never sent");
        answers.set_text("answer", " inspected result ");
        let result = answers.complete(&[q.clone()]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].labels, ["inspected result"]);
        assert!(answers.complete(&[q.clone(), q]).is_none());
        assert!(answers.complete(&[]).is_none());
    }
}
