pub(crate) const VISIBLE_PROCESS_NAMES: &[&str] = &[
    "unrealeditor",
    "unrealeditor-cmd",
    "ue4editor",
    "ue4editor-cmd",
    "shadercompileworker",
    "unrealbuildtool",
    "unrealpak",
    "automationtool",
    "crashreportclienteditor",
    "blender",
    "blender-launcher",
    "msbuild",
    "devenv",
    "cmake",
    "ninja",
    "cl",
    "link",
    "rc",
];

pub(crate) fn is_visible_process_name(name: &str) -> bool {
    let normalized = normalize_process_name(name);
    VISIBLE_PROCESS_NAMES
        .iter()
        .any(|allowed| normalized.eq_ignore_ascii_case(allowed))
}

pub(crate) fn public_process_details() -> String {
    "task-relevant development process".to_owned()
}

fn normalize_process_name(name: &str) -> &str {
    name.trim()
        .strip_suffix(".exe")
        .or_else(|| name.trim().strip_suffix(".EXE"))
        .unwrap_or_else(|| name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_game_development_processes() {
        assert!(is_visible_process_name("UnrealEditor.exe"));
        assert!(is_visible_process_name("ShaderCompileWorker.exe"));
        assert!(is_visible_process_name("blender.exe"));
        assert!(is_visible_process_name("MSBuild.exe"));
    }

    #[test]
    fn hides_unrelated_desktop_and_background_processes() {
        for name in [
            "explorer.exe",
            "chrome.exe",
            "ChatGPT.exe",
            "powershell.exe",
            "svchost.exe",
            "MsMpEng.exe",
            "NVDisplay.Container.exe",
        ] {
            assert!(!is_visible_process_name(name), "{name} must stay hidden");
        }
    }
}
