use serde_json::{Value, json};

pub(super) fn is_plan_mode_request(content: &str) -> bool {
    let normalized = intent_prose(content)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let negated = [
        "do not plan",
        "don't plan",
        "no plan needed",
        "skip planning",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));
    !negated
        && (normalized.contains("plan this")
            || normalized.contains("make a plan")
            || normalized.contains("create a plan")
            || normalized.split_whitespace().any(|word| word == "#plan"))
}

pub(super) fn intent_hint(content: &str) -> Value {
    let normalized = intent_prose(content).to_lowercase();
    let workflow_kind = if is_plan_mode_request(content) {
        "plan"
    } else if ["review only", "do not edit", "don't edit"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        "review"
    } else if ["debug", "fix bug", "fix the bug"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        "debug"
    } else if ["commit", "create commit"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        "commit"
    } else {
        "implement"
    };
    json!({
        "workflowKind": workflow_kind,
        "authoritative": false,
        "grantsExecutionPermission": false,
        "note": "Language classification is a workflow hint only; effective effects come from task policy and authenticated user decisions."
    })
}

fn intent_prose(content: &str) -> String {
    let mut prose = String::with_capacity(content.len());
    let mut fenced = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut delimiter = None;
        for character in line.chars() {
            if matches!(character, '`' | '"' | '\'') {
                delimiter = match delimiter {
                    Some(active) if active == character => None,
                    None => Some(character),
                    active => active,
                };
            } else if delimiter.is_none() {
                prose.push(character);
            }
        }
        prose.push(' ');
    }
    prose
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_trigger_ignores_negation_quotes_and_code() {
        assert!(is_plan_mode_request("Plan this gift purchase"));
        assert!(is_plan_mode_request(
            "MAKE   A   PLAN\nfor the sales website"
        ));
        assert!(is_plan_mode_request("Build the website for me #PLAN"));
        assert!(!is_plan_mode_request("Show me the current roadmap"));
        assert!(!is_plan_mode_request("Use the planner to track work"));
        assert!(!is_plan_mode_request("No plan needed, edit it now"));
        assert!(!is_plan_mode_request("Do not plan; implement directly"));
        assert!(!is_plan_mode_request(
            "The log contains `#plan` but fix the bug"
        ));
        assert!(!is_plan_mode_request(
            "Example:\n```text\nmake a plan\n```\nEdit the code"
        ));
        assert!(!is_plan_mode_request("Review the string \"plan this\""));
    }

    #[test]
    fn intent_hint_never_grants_execution_permission() {
        let review = intent_hint("Review only, don't edit");
        assert_eq!(review["workflowKind"], "review");
        assert_eq!(review["authoritative"], false);
        assert_eq!(review["grantsExecutionPermission"], false);
    }
}
