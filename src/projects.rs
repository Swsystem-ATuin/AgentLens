use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{cmp::Reverse, path::Path};

const MAX_NAME_CHARS: usize = 120;
const MAX_PROJECT_RULES: usize = 64;
const MAX_PATH_CHARS: usize = 4_096;

#[derive(Clone, Debug)]
pub(crate) struct ProjectConfig {
    timezone: String,
    rules: Vec<ProjectRule>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectRule {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) label: String,
    pub(crate) windows: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default = "default_timezone")]
    timezone: String,
    #[serde(default)]
    projects: Vec<ProjectDefinition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectDefinition {
    name: String,
    host: String,
    root: String,
    #[serde(default)]
    subprojects: Vec<SubprojectDefinition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubprojectDefinition {
    name: String,
    path: String,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            timezone: default_timezone(),
            rules: Vec::new(),
        }
    }
}

impl ProjectConfig {
    pub(crate) fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let bytes = std::fs::read(path)
            .with_context(|| format!("read project configuration {}", path.display()))?;
        let file: ConfigFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse project configuration {}", path.display()))?;
        Self::from_file(file).context("validate project configuration")
    }

    fn from_file(file: ConfigFile) -> Result<Self> {
        let timezone = validate_timezone(&file.timezone)?;
        let mut rules = Vec::new();
        for project in file.projects {
            validate_name(&project.name, "project name")?;
            ensure!(
                !project.host.chars().any(char::is_control),
                "project host contains a control character"
            );
            let windows = is_windows_path(&project.root);
            ensure!(
                project.root.chars().count() <= MAX_PATH_CHARS,
                "project root is too long"
            );
            let root = normalize_absolute_path(&project.root, windows)
                .with_context(|| format!("invalid root for project {}", project.name))?;
            rules.push(ProjectRule {
                host: project.host.clone(),
                path: root.clone(),
                label: project.name.clone(),
                windows,
            });
            for subproject in project.subprojects {
                validate_name(&subproject.name, "subproject name")?;
                ensure!(
                    subproject.path.chars().count() <= MAX_PATH_CHARS,
                    "subproject path is too long"
                );
                ensure!(
                    !is_absolute_path(&subproject.path),
                    "subproject path must be relative"
                );
                let combined = format!("{root}/{}", subproject.path.replace('\\', "/"));
                let path = normalize_absolute_path(&combined, windows).with_context(|| {
                    format!(
                        "invalid path for subproject {} / {}",
                        project.name, subproject.name
                    )
                })?;
                ensure!(
                    path.starts_with(&format!("{root}/")),
                    "subproject path must remain beneath its project root"
                );
                rules.push(ProjectRule {
                    host: project.host.clone(),
                    path,
                    label: format!("{} / {}", project.name, subproject.name),
                    windows,
                });
                ensure!(
                    rules.len() <= MAX_PROJECT_RULES,
                    "project configuration has more than {MAX_PROJECT_RULES} roots and subprojects"
                );
            }
            ensure!(
                rules.len() <= MAX_PROJECT_RULES,
                "project configuration has more than {MAX_PROJECT_RULES} roots and subprojects"
            );
        }
        rules.sort_by_key(|rule| Reverse(rule.path.len()));
        for (index, rule) in rules.iter().enumerate() {
            if rules[..index].iter().any(|candidate| {
                candidate.host == rule.host
                    && candidate.path == rule.path
                    && candidate.label != rule.label
            }) {
                bail!("one host/path is assigned to multiple project labels");
            }
        }
        Ok(Self { timezone, rules })
    }

    pub(crate) fn timezone(&self) -> &str {
        &self.timezone
    }

    pub(crate) fn rules(&self) -> &[ProjectRule] {
        &self.rules
    }

    #[cfg(test)]
    pub(crate) fn classify(&self, host: &str, cwd: Option<&str>) -> Option<&str> {
        let cwd = cwd?;
        self.rules.iter().find_map(|rule| {
            if rule.host != host {
                return None;
            }
            let normalized = normalize_absolute_path(cwd, rule.windows).ok()?;
            (normalized == rule.path || normalized.starts_with(&format!("{}/", rule.path)))
                .then_some(rule.label.as_str())
        })
    }
}

fn default_timezone() -> String {
    "+00:00".to_owned()
}

fn validate_timezone(value: &str) -> Result<String> {
    if value.eq_ignore_ascii_case("UTC") || value == "Z" {
        return Ok(default_timezone());
    }
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 6
            && matches!(bytes[0], b'+' | b'-')
            && bytes[1..3].iter().all(u8::is_ascii_digit)
            && bytes[3] == b':'
            && bytes[4..6].iter().all(u8::is_ascii_digit),
        "timezone must be UTC, Z, or a fixed offset such as +01:00"
    );
    let hours = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
    let minutes = (bytes[4] - b'0') * 10 + bytes[5] - b'0';
    ensure!(
        hours <= 14 && minutes < 60,
        "timezone offset is out of range"
    );
    ensure!(
        hours != 14 || minutes == 0,
        "timezone offset is out of range"
    );
    Ok(value.to_owned())
}

fn validate_name(value: &str, field: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    ensure!(
        value.chars().count() <= MAX_NAME_CHARS,
        "{field} is too long"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "{field} contains a control character"
    );
    Ok(())
}

fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':') || path.contains('\\')
}

fn is_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\')
}

fn normalize_absolute_path(path: &str, windows: bool) -> Result<String> {
    let replaced = path.replace('\\', "/");
    let (prefix, remainder) = if windows {
        let bytes = replaced.as_bytes();
        ensure!(
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/',
            "Windows paths must use an absolute drive path"
        );
        (replaced[..2].to_ascii_lowercase(), &replaced[3..])
    } else {
        ensure!(replaced.starts_with('/'), "POSIX paths must be absolute");
        (String::new(), replaced.trim_start_matches('/'))
    };
    let mut components = Vec::new();
    for component in remainder.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                ensure!(components.pop().is_some(), "path escapes its root");
            }
            component => components.push(if windows {
                component.to_ascii_lowercase()
            } else {
                component.to_owned()
            }),
        }
    }
    ensure!(!components.is_empty(), "filesystem roots are too broad");
    if windows {
        Ok(format!("{prefix}/{}", components.join("/")))
    } else {
        Ok(format!("/{}", components.join("/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> ProjectConfig {
        ProjectConfig::from_file(serde_json::from_str(json).unwrap()).unwrap()
    }

    #[test]
    fn longest_host_specific_root_wins_for_posix_and_windows_paths() {
        let projects = config(
            r#"{
                "timezone":"+01:00",
                "projects":[
                    {"name":"Mono","host":"linux","root":"/work/mono","subprojects":[{"name":"API","path":"services/api"}]},
                    {"name":"Win","host":"windows","root":"C:\\Work\\Repo","subprojects":[{"name":"UI","path":"Apps\\UI"}]}
                ]
            }"#,
        );
        assert_eq!(
            projects.classify("linux", Some("/work/mono/services/api/src")),
            Some("Mono / API")
        );
        assert_eq!(
            projects.classify("linux", Some("/work/mono/other")),
            Some("Mono")
        );
        assert_eq!(
            projects.classify("windows", Some("c:/WORK/repo/apps/ui/src")),
            Some("Win / UI")
        );
        assert_eq!(projects.classify("other", Some("/work/mono")), None);
        assert_eq!(projects.classify("linux", Some("/work/monolith")), None);
        assert_eq!(projects.classify("linux", None), None);
    }

    #[test]
    fn rejects_invalid_timezone_roots_and_escaping_subprojects() {
        for input in [
            r#"{"timezone":"Europe/London"}"#,
            r#"{"projects":[{"name":"Bad","host":"h","root":"relative"}]}"#,
            r#"{"projects":[{"name":"Bad","host":"h","root":"/repo","subprojects":[{"name":"Escape","path":"../other"}]}]}"#,
        ] {
            let file: ConfigFile = serde_json::from_str(input).unwrap();
            assert!(ProjectConfig::from_file(file).is_err(), "{input}");
        }
    }

    #[test]
    fn checked_in_example_is_valid() {
        let file: ConfigFile =
            serde_json::from_str(include_str!("../projects.example.json")).unwrap();
        let config = ProjectConfig::from_file(file).unwrap();
        assert_eq!(config.timezone(), "+00:00");
        assert_eq!(config.rules().len(), 2);
    }
}
