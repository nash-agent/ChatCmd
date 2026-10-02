use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::Value;

use super::RuntimeHost;

const PROJECT_ALIAS: &str = "@project";
const RUNTIME_ALIAS: &str = "@runtime";

#[derive(Clone, Debug)]
pub(super) struct VirtualWorkspaceView {
    roots: Vec<VirtualRoot>,
}

#[derive(Clone, Debug)]
struct VirtualRoot {
    alias: String,
    physical: PathBuf,
    variants: Vec<String>,
}

impl RuntimeHost {
    pub(super) async fn virtual_workspace_view(
        &self,
        context: &OperationContext,
    ) -> VirtualWorkspaceView {
        let project_folder =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await
                .ok()
                .flatten()
                .map(PathBuf::from);

        let mut roots = self.workspace.roots().to_vec();
        match self.task_user_path_scopes(context).await {
            Ok(scopes) => roots.extend(scopes),
            Err(error) => {
                tracing::warn!(
                    code = %error.code,
                    "virtual workspace path scopes unavailable; configured roots remain masked"
                );
            }
        }
        if let Some(project) = project_folder.as_ref()
            && !roots.iter().any(|root| same_path(root, project))
        {
            roots.push(project.clone());
        }

        VirtualWorkspaceView::new(project_folder, roots)
    }
}

impl VirtualWorkspaceView {
    pub(super) fn new(project_folder: Option<PathBuf>, roots: Vec<PathBuf>) -> Self {
        let project_folder = project_folder.map(canonical_or_original);

        let mut unique = BTreeMap::<String, PathBuf>::new();
        for root in roots {
            let root = canonical_or_original(root);
            unique.entry(path_key(&root)).or_insert(root);
        }

        let mut projected = Vec::new();
        if let Some(project) = project_folder.as_ref() {
            unique.remove(&path_key(project));
            projected.push(VirtualRoot::new(PROJECT_ALIAS.to_owned(), project.clone()));
        }

        let mut remaining = unique.into_values().collect::<Vec<_>>();
        remaining.sort_by(|left, right| {
            alias_base(left)
                .cmp(&alias_base(right))
                .then_with(|| path_key(left).cmp(&path_key(right)))
        });
        let mut used = BTreeSet::new();
        used.insert(PROJECT_ALIAS.to_owned());
        for root in remaining {
            let base = alias_base(&root);
            let mut alias = format!("@{base}");
            let mut suffix = 2_u32;
            while used.contains(&alias) {
                alias = format!("@{base}-{suffix}");
                suffix = suffix.saturating_add(1);
            }
            used.insert(alias.clone());
            projected.push(VirtualRoot::new(alias, root));
        }

        Self { roots: projected }
    }

    pub(super) fn aliases(&self) -> Vec<String> {
        self.roots.iter().map(|root| root.alias.clone()).collect()
    }

    pub(super) fn resolve_alias_arguments(
        &self,
        tool: &str,
        mut arguments: Value,
    ) -> RuntimeResult<Value> {
        let Some(object) = arguments.as_object_mut() else {
            return Ok(arguments);
        };

        if tool.starts_with("fs_") || tool.starts_with("workspace_index_") {
            for key in ["path", "source", "destination", "quarantinePath"] {
                if let Some(value) = object.get_mut(key) {
                    self.resolve_alias_value(value)?;
                }
            }
            if let Some(paths) = object.get_mut("paths").and_then(Value::as_array_mut) {
                for value in paths {
                    self.resolve_alias_value(value)?;
                }
            }
            if let Some(requests) = object.get_mut("requests").and_then(Value::as_array_mut) {
                for request in requests {
                    if let Some(path) = request
                        .as_object_mut()
                        .and_then(|request| request.get_mut("path"))
                    {
                        self.resolve_alias_value(path)?;
                    }
                }
            }
            return Ok(arguments);
        }

        if tool.starts_with("git_") {
            if let Some(cwd) = object.get_mut("cwd") {
                self.resolve_alias_value(cwd)?;
            }
            return Ok(arguments);
        }

        if tool == "shell_create" {
            for key in [
                "workingDirectory",
                "cwd",
                "initialWorkingDirectory",
                "executable",
            ] {
                if let Some(value) = object.get_mut(key) {
                    self.resolve_alias_value(value)?;
                }
            }
            return Ok(arguments);
        }

        if tool == "command_run" {
            for key in ["cwd", "executable"] {
                if let Some(value) = object.get_mut(key) {
                    self.resolve_alias_value(value)?;
                }
            }
            return Ok(arguments);
        }

        if tool == "project_context" {
            if let Some(paths) = object.get_mut("targetPaths").and_then(Value::as_array_mut) {
                for value in paths {
                    self.resolve_alias_value(value)?;
                }
            }
            if let Some(path) = object
                .get_mut("range")
                .and_then(Value::as_object_mut)
                .and_then(|range| range.get_mut("path"))
            {
                self.resolve_alias_value(path)?;
            }
        }

        Ok(arguments)
    }

    pub(super) fn project_tool_output(&self, tool: &str, mut value: Value) -> Value {
        self.project_value(tool, None, &mut value);
        value
    }

    pub(super) fn project_error(&self, error: &mut RuntimeError) {
        error.message = self.project_embedded_text(&error.message);
    }

    pub(super) fn project_path(&self, path: &Path) -> String {
        self.project_path_text(&path.to_string_lossy())
    }

    fn project_value(&self, tool: &str, field: Option<&str>, value: &mut Value) {
        match value {
            Value::String(text) => {
                if field.is_some_and(is_path_field) {
                    *text = self.project_path_text(text);
                } else if field.is_some_and(|field| is_text_field(tool, field)) {
                    *text = self.project_embedded_text(text);
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.project_value(tool, field, value);
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    self.project_value(tool, Some(key.as_str()), value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    fn resolve_alias_value(&self, value: &mut Value) -> RuntimeResult<()> {
        let Some(raw) = value.as_str() else {
            return Ok(());
        };
        let Some(path) = self.resolve_alias(raw)? else {
            return Ok(());
        };
        *value = Value::String(path.to_string_lossy().into_owned());
        Ok(())
    }

    fn resolve_alias(&self, raw: &str) -> RuntimeResult<Option<PathBuf>> {
        for root in &self.roots {
            if raw == root.alias {
                return Ok(Some(root.physical.clone()));
            }
            for separator in ['/', '\\'] {
                let prefix = format!("{}{separator}", root.alias);
                if let Some(rest) = raw.strip_prefix(&prefix) {
                    if rest.split(['/', '\\']).any(|segment| segment == "..") {
                        return Err(RuntimeError::new(
                            "invalid_virtual_path",
                            "virtual workspace paths cannot contain '..'",
                        ));
                    }
                    let mut path = root.physical.clone();
                    for segment in rest
                        .split(['/', '\\'])
                        .filter(|segment| !segment.is_empty())
                    {
                        if segment != "." {
                            path.push(segment);
                        }
                    }
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }

    fn project_path_text(&self, text: &str) -> String {
        let projected = self.project_embedded_text(text);
        if projected.starts_with('@') {
            projected.replace('\\', "/")
        } else {
            projected
        }
    }

    fn project_embedded_text(&self, text: &str) -> String {
        let mut replacements = self
            .roots
            .iter()
            .flat_map(|root| {
                root.variants
                    .iter()
                    .map(move |variant| (variant.as_str(), root.alias.as_str()))
            })
            .collect::<Vec<_>>();
        replacements.sort_by(|left, right| right.0.len().cmp(&left.0.len()));

        let mut output = text.to_owned();
        for (needle, alias) in replacements {
            output = replace_path_root(&output, needle, alias);
        }
        mask_windows_absolute_prefixes(&output)
    }
}

impl VirtualRoot {
    fn new(alias: String, physical: PathBuf) -> Self {
        Self {
            alias,
            variants: path_variants(&physical),
            physical,
        }
    }
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn same_path(left: &Path, right: &Path) -> bool {
    path_key(&canonical_or_original(left.to_path_buf()))
        == path_key(&canonical_or_original(right.to_path_buf()))
}

fn path_key(path: &Path) -> String {
    let text = strip_verbatim_prefix(&path.to_string_lossy()).replace('\\', "/");
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}

fn alias_base(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .trim_matches('.');
    let mut slug = String::with_capacity(raw.len());
    let mut last_dash = false;
    for character in raw.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "workspace".to_owned()
    } else {
        slug.to_owned()
    }
}

fn path_variants(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy().into_owned();
    let plain = strip_verbatim_prefix(&raw);
    let mut variants = BTreeSet::new();
    variants.insert(raw.clone());
    variants.insert(plain.clone());
    variants.insert(raw.replace('\\', "/"));
    variants.insert(plain.replace('\\', "/"));
    variants
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn strip_verbatim_prefix(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}

fn replace_path_root(text: &str, needle: &str, alias: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }

    let mut output = String::with_capacity(text.len());
    let mut remainder = text;
    while let Some(index) = find_path_root(remainder, needle) {
        let before = &remainder[..index];
        if let Some(stripped) = before.strip_suffix(r"\\?\") {
            output.push_str(stripped);
        } else if let Some(stripped) = before.strip_suffix("//?/") {
            output.push_str(stripped);
        } else {
            output.push_str(before);
        }
        let after = &remainder[index + needle.len()..];
        let bounded = after.is_empty()
            || after.starts_with('/')
            || after.starts_with('\\')
            || after.starts_with('"')
            || after.starts_with('\'')
            || after.starts_with(':')
            || after.chars().next().is_some_and(|character| {
                character.is_whitespace()
                    || matches!(character, '>' | '<' | ')' | ']' | '}' | ',' | ';')
            });

        if bounded {
            output.push_str(alias);
            remainder = after;
        } else {
            output.push_str(&remainder[..index + needle.len()]);
            remainder = &remainder[index + needle.len()..];
        }
    }
    output.push_str(remainder);
    output
}

fn find_path_root(text: &str, needle: &str) -> Option<usize> {
    if cfg!(windows) {
        text.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
    } else {
        text.find(needle)
    }
}

fn mask_windows_absolute_prefixes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < bytes.len() {
        let verbatim = index + 7 <= bytes.len()
            && matches!(
                &bytes[index..index + 4],
                [b'\\', b'\\', b'?', b'\\'] | [b'/', b'/', b'?', b'/']
            )
            && bytes[index + 4].is_ascii_alphabetic()
            && bytes[index + 5] == b':'
            && matches!(bytes[index + 6], b'\\' | b'/');
        if verbatim {
            output.push_str(RUNTIME_ALIAS);
            output.push('/');
            index += 7;
            continue;
        }

        let drive_boundary = index == 0
            || !bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_';
        let drive = drive_boundary
            && index + 3 <= bytes.len()
            && bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && matches!(bytes[index + 2], b'\\' | b'/');
        if drive {
            output.push_str(RUNTIME_ALIAS);
            output.push('/');
            index += 3;
            continue;
        }

        let character = text[index..]
            .chars()
            .next()
            .expect("valid UTF-8 character boundary");
        output.push(character);
        index += character.len_utf8();
    }

    output
}

fn is_path_field(field: &str) -> bool {
    matches!(
        field,
        "path"
            | "paths"
            | "source"
            | "destination"
            | "cwd"
            | "workingDirectory"
            | "initialWorkingDirectory"
            | "root"
            | "rootPath"
            | "roots"
            | "projectFolder"
            | "imagePath"
            | "artifactPath"
            | "quarantinePath"
            | "executable"
            | "executablePath"
    )
}

fn is_text_field(tool: &str, field: &str) -> bool {
    if matches!(field, "stderr" | "warning" | "warnings") {
        return true;
    }
    if tool == "command_run" {
        return matches!(field, "stdout" | "text" | "message" | "commandLine");
    }
    if tool.starts_with("shell_") {
        return matches!(
            field,
            "stdout" | "text" | "message" | "commandLine" | "data"
        );
    }
    if tool.starts_with("process_") {
        return matches!(
            field,
            "stdout" | "text" | "message" | "commandLine" | "details"
        );
    }
    false
}

#[cfg(test)]
#[path = "virtual_workspace_tests.rs"]
mod tests;
