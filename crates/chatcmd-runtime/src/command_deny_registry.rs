use crate::{RuntimeError, RuntimeResult};
use std::path::Path;

pub(crate) struct DeniedCommandRule {
    pub(crate) id: &'static str,
    matcher: fn(&str, &[String], Option<&str>) -> bool,
}

pub(crate) const DENIED_COMMAND_RULES: &[DeniedCommandRule] = &[
    DeniedCommandRule {
        id: "process_inventory_tasklist",
        matcher: matches_tasklist,
    },
    DeniedCommandRule {
        id: "process_inventory_ps",
        matcher: matches_ps,
    },
    DeniedCommandRule {
        id: "process_inventory_wmic",
        matcher: matches_wmic_process,
    },
    DeniedCommandRule {
        id: "process_inventory_powershell",
        matcher: matches_powershell_process_inventory,
    },
];

pub(crate) fn validate_spawn(executable: &str, arguments: &[String]) -> RuntimeResult<()> {
    validate(executable, arguments, None)
}

pub(crate) fn validate_shell_input(text: &str) -> RuntimeResult<()> {
    validate("", &[], Some(text))
}

fn validate(executable: &str, arguments: &[String], shell_text: Option<&str>) -> RuntimeResult<()> {
    for rule in DENIED_COMMAND_RULES {
        if (rule.matcher)(executable, arguments, shell_text) {
            return Err(RuntimeError::new(
                "command_policy_denied",
                format!(
                    "command is blocked by execution-runtime policy ({})",
                    rule.id
                ),
            ));
        }
    }
    Ok(())
}

fn executable_name(executable: &str) -> String {
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(executable)
        .trim()
        .to_ascii_lowercase()
}

fn command_text(arguments: &[String], shell_text: Option<&str>) -> String {
    shell_text.map_or_else(
        || arguments.join(" ").to_ascii_lowercase(),
        |text| text.to_ascii_lowercase(),
    )
}

fn matches_tasklist(executable: &str, arguments: &[String], shell_text: Option<&str>) -> bool {
    let name = executable_name(executable);
    if matches!(name.as_str(), "tasklist" | "tasklist.exe") {
        return true;
    }
    contains_command_word(&command_text(arguments, shell_text), "tasklist")
}

fn matches_ps(executable: &str, arguments: &[String], shell_text: Option<&str>) -> bool {
    let name = executable_name(executable);
    if matches!(name.as_str(), "ps" | "ps.exe") {
        return true;
    }
    let text = command_text(arguments, shell_text);
    is_command_line(&text, "ps")
}

fn matches_wmic_process(executable: &str, arguments: &[String], shell_text: Option<&str>) -> bool {
    let name = executable_name(executable);
    let text = command_text(arguments, shell_text);
    if matches!(name.as_str(), "wmic" | "wmic.exe") {
        return contains_command_word(&text, "process")
            || contains_command_word(&text, "win32_process");
    }
    contains_command_word(&text, "wmic")
        && (contains_command_word(&text, "process")
            || contains_command_word(&text, "win32_process"))
}

fn matches_powershell_process_inventory(
    _executable: &str,
    arguments: &[String],
    shell_text: Option<&str>,
) -> bool {
    let text = command_text(arguments, shell_text);

    if contains_command_word(&text, "get-process") || contains_command_word(&text, "gps") {
        // Targeted PID inspection is allowed; inventory-style enumeration is not.
        if contains_command_word(&text, "-id") || contains_command_word(&text, "-pid") {
            return false;
        }
        return true;
    }

    if (contains_command_word(&text, "get-ciminstance")
        || contains_command_word(&text, "gcim")
        || contains_command_word(&text, "get-wmiobject")
        || contains_command_word(&text, "gwmi"))
        && contains_command_word(&text, "win32_process")
    {
        return true;
    }

    text.contains("system.diagnostics.process")
        && (text.contains("getprocesses(") || text.contains("getprocessesbyname("))
}

fn contains_command_word(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(start, _)| {
        let end = start + needle.len();
        let left_ok = start == 0 || !text[..start].chars().next_back().is_some_and(is_word_char);
        let right_ok = end == text.len() || !text[end..].chars().next().is_some_and(is_word_char);
        left_ok && right_ok
    })
}

fn is_command_line(text: &str, command: &str) -> bool {
    text.split(['\r', '\n', ';', '|', '&'])
        .map(str::trim_start)
        .any(|segment| {
            segment == command
                || segment
                    .strip_prefix(command)
                    .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        })
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn blocks_direct_process_inventory_commands() {
        assert!(validate_spawn("tasklist.exe", &[]).is_err());
        assert!(validate_spawn("ps", &args(&["-eo", "pid,comm"])).is_err());
        assert!(validate_spawn("wmic.exe", &args(&["process", "list"])).is_err());
    }

    #[test]
    fn blocks_shell_process_inventory_commands() {
        assert!(validate_shell_input("Get-Process").is_err());
        assert!(validate_shell_input("gps | Sort-Object CPU").is_err());
        assert!(validate_shell_input("Get-CimInstance Win32_Process").is_err());
        assert!(validate_shell_input("cmd /c tasklist").is_err());
    }

    #[test]
    fn allows_targeted_get_process_by_pid() {
        assert!(validate_shell_input("Get-Process -Id 1234").is_ok());
        assert!(
            validate_spawn(
                "powershell.exe",
                &args(&["-NoProfile", "-Command", "Get-Process -Id 1234"])
            )
            .is_ok()
        );
    }

    #[test]
    fn allows_unrelated_shell_commands() {
        assert!(validate_shell_input("Get-ChildItem .").is_ok());
        assert!(
            validate_spawn(
                "powershell.exe",
                &args(&["-NoProfile", "-Command", "Write-Output PASS"])
            )
            .is_ok()
        );
    }
}
