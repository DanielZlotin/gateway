use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_DEVELOPER_INSTRUCTIONS_BYTES: usize = 256 * 1024;

const SYSTEM_PROMPT: &str = include_str!("prompts/SYSTEM.md");
const AGENTS_TEMPLATE: &str = include_str!("prompts/AGENTS.md");
const IDENTITY_TEMPLATE: &str = include_str!("prompts/IDENTITY.md");
const USER_TEMPLATE: &str = include_str!("prompts/USER.md");
const TOOLS_TEMPLATE: &str = include_str!("prompts/TOOLS.md");
const MEMORY_TEMPLATE: &str = include_str!("prompts/MEMORY.md");
const HEARTBEAT_TEMPLATE: &str = include_str!("prompts/HEARTBEAT.md");

struct ContextFile {
    name: &'static str,
    template: &'static str,
}

const CORE_CONTEXT_FILES: &[ContextFile] = &[
    ContextFile {
        name: "AGENTS.md",
        template: AGENTS_TEMPLATE,
    },
    ContextFile {
        name: "IDENTITY.md",
        template: IDENTITY_TEMPLATE,
    },
    ContextFile {
        name: "USER.md",
        template: USER_TEMPLATE,
    },
    ContextFile {
        name: "TOOLS.md",
        template: TOOLS_TEMPLATE,
    },
    ContextFile {
        name: "MEMORY.md",
        template: MEMORY_TEMPLATE,
    },
];

const HEARTBEAT_CONTEXT_FILE: ContextFile = ContextFile {
    name: "HEARTBEAT.md",
    template: HEARTBEAT_TEMPLATE,
};

pub fn ensure_gateway_context_files(xdg_config_home: &Path) -> Result<(), String> {
    for file in CORE_CONTEXT_FILES
        .iter()
        .chain(std::iter::once(&HEARTBEAT_CONTEXT_FILE))
    {
        ensure_context_file(xdg_config_home, file)?;
    }
    Ok(())
}

pub fn developer_instructions(xdg_config_home: &Path) -> Result<String, String> {
    let mut parts = Vec::with_capacity(CORE_CONTEXT_FILES.len() + 1);
    parts.push(SYSTEM_PROMPT.trim_end().to_string());

    let mut largest_file = None;
    for file in CORE_CONTEXT_FILES {
        let text = read_context_file(xdg_config_home, file)?;
        if largest_file
            .map(|(_, byte_len)| text.len() > byte_len)
            .unwrap_or(true)
        {
            largest_file = Some((file.name, text.len()));
        }
        parts.push(text.trim_end().to_string());
    }

    let instructions = parts.join("\n\n");
    if instructions.len() > MAX_DEVELOPER_INSTRUCTIONS_BYTES {
        let name = largest_file
            .map(|(name, _)| name)
            .unwrap_or("unknown context file");
        return Err(format!(
            "gateway core context is too large: {} bytes exceeds {} byte limit; likely largest file: {name}; trim $XDG_CONFIG_HOME/gateway/{name}",
            instructions.len(),
            MAX_DEVELOPER_INSTRUCTIONS_BYTES,
        ));
    }

    Ok(instructions)
}

pub fn ensure_heartbeat_prompt_file(xdg_config_home: &Path) -> Result<PathBuf, String> {
    ensure_context_file(xdg_config_home, &HEARTBEAT_CONTEXT_FILE)
}

fn ensure_context_file(xdg_config_home: &Path, file: &ContextFile) -> Result<PathBuf, String> {
    let path = context_file_path(xdg_config_home, file.name);
    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(format!(
                "read gateway context file {}: {err}",
                path.display()
            ))
        }
    };
    let next = refreshed_context_text(file.template, existing.as_deref())?;
    if existing.as_deref() != Some(next.as_str()) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("create gateway context dir {}: {err}", parent.display()))?;
        }
        fs::write(&path, next)
            .map_err(|err| format!("write gateway context file {}: {err}", path.display()))?;
    }
    Ok(path)
}

fn read_context_file(xdg_config_home: &Path, file: &ContextFile) -> Result<String, String> {
    let path = context_file_path(xdg_config_home, file.name);
    fs::read_to_string(&path)
        .map_err(|err| format!("read gateway context file {}: {err}", path.display()))
}

fn context_file_path(xdg_config_home: &Path, name: &str) -> PathBuf {
    xdg_config_home.join("gateway").join(name)
}

fn refreshed_context_text(template: &str, existing: Option<&str>) -> Result<String, String> {
    let header = template_header(template)?;
    let Some(existing) = existing else {
        return Ok(header);
    };
    Ok(format!("{}{}", header, body_after_line_two(existing)))
}

fn template_header(template: &str) -> Result<String, String> {
    let mut lines = template.lines();
    let first = lines
        .next()
        .ok_or_else(|| "embedded context template is malformed: missing first line".to_string())?;
    if !first.starts_with("# ") {
        return Err(
            "embedded context template is malformed: first line must start with `# `".to_string(),
        );
    }

    let second = lines
        .next()
        .ok_or_else(|| "embedded context template is malformed: missing second line".to_string())?;
    if !second.starts_with("> **Scope:**") {
        return Err(
            "embedded context template is malformed: second line must start with `> **Scope:**`"
                .to_string(),
        );
    }

    Ok(format!("{first}\n{second}\n"))
}

fn body_after_line_two(text: &str) -> &str {
    let mut line_count = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            line_count += 1;
            if line_count == 2 {
                return &text[index + 1..];
            }
        }
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn system_template_is_loaded_from_context_module() {
        assert!(SYSTEM_PROMPT.contains("Gateway Runtime Instructions"));
    }

    #[test]
    fn system_template_blocks_private_data_in_telegram() {
        assert!(SYSTEM_PROMPT.contains("Telegram"));
        assert!(SYSTEM_PROMPT.contains("environment variables"));
        assert!(SYSTEM_PROMPT.contains("private keys"));
    }

    #[test]
    fn ensure_gateway_context_files_creates_core_and_heartbeat_files() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");

        ensure_gateway_context_files(&xdg_config_home).unwrap();

        for file in CORE_CONTEXT_FILES
            .iter()
            .chain(std::iter::once(&HEARTBEAT_CONTEXT_FILE))
        {
            let path = xdg_config_home.join("gateway").join(file.name);
            assert!(path.exists(), "missing {}", file.name);
            assert_eq!(
                fs::read_to_string(path).unwrap(),
                valid_template_header(file.template)
            );
        }
    }

    #[test]
    fn ensure_gateway_context_files_refreshes_header_and_preserves_body() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");
        let gateway_dir = xdg_config_home.join("gateway");
        fs::create_dir_all(&gateway_dir).unwrap();
        let agents_path = gateway_dir.join("AGENTS.md");
        fs::write(
            &agents_path,
            "# Old header\n> Old scope\n\nKeep this body.\nAnd this line.\n",
        )
        .unwrap();

        ensure_gateway_context_files(&xdg_config_home).unwrap();

        assert_eq!(
            fs::read_to_string(agents_path).unwrap(),
            format!(
                "{}\nKeep this body.\nAnd this line.\n",
                valid_template_header(AGENTS_TEMPLATE)
            )
        );
    }

    #[test]
    fn developer_instructions_include_core_files_in_order_and_exclude_heartbeat() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");
        ensure_gateway_context_files(&xdg_config_home).unwrap();

        for file in CORE_CONTEXT_FILES {
            fs::write(
                xdg_config_home.join("gateway").join(file.name),
                format!(
                    "{}{} body\n",
                    valid_template_header(file.template),
                    file.name
                ),
            )
            .unwrap();
        }
        fs::write(
            xdg_config_home.join("gateway/HEARTBEAT.md"),
            format!(
                "{}heartbeat-only body\n",
                valid_template_header(HEARTBEAT_TEMPLATE)
            ),
        )
        .unwrap();

        let instructions = developer_instructions(&xdg_config_home).unwrap();

        assert!(instructions.starts_with(SYSTEM_PROMPT.trim_end()));
        assert_in_order(
            &instructions,
            &[
                "# 🌉 Gateway Runtime Instructions",
                "# AGENTS.md",
                "# IDENTITY.md",
                "# USER.md",
                "# TOOLS.md",
                "# MEMORY.md",
            ],
        );
        assert!(!instructions.contains("# HEARTBEAT.md"));
        assert!(!instructions.contains("heartbeat-only body"));
    }

    #[test]
    fn developer_instructions_fail_when_required_core_file_is_missing() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");
        ensure_gateway_context_files(&xdg_config_home).unwrap();
        let missing_path = xdg_config_home.join("gateway/USER.md");
        fs::remove_file(&missing_path).unwrap();

        let err = developer_instructions(&xdg_config_home).unwrap_err();

        assert!(err.contains("read gateway context file"), "{err}");
        assert!(err.contains("USER.md"), "{err}");
        assert!(!missing_path.exists());
    }

    #[test]
    fn developer_instructions_fail_when_core_context_is_too_large() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");
        ensure_gateway_context_files(&xdg_config_home).unwrap();
        fs::write(
            xdg_config_home.join("gateway/MEMORY.md"),
            format!(
                "{}{}",
                valid_template_header(MEMORY_TEMPLATE),
                "x".repeat(MAX_DEVELOPER_INSTRUCTIONS_BYTES)
            ),
        )
        .unwrap();
        let expected_len = {
            let mut parts = vec![SYSTEM_PROMPT.trim_end().to_string()];
            for file in CORE_CONTEXT_FILES {
                let text =
                    fs::read_to_string(xdg_config_home.join("gateway").join(file.name)).unwrap();
                parts.push(text.trim_end().to_string());
            }
            parts.join("\n\n").len()
        };

        let err = developer_instructions(&xdg_config_home).unwrap_err();

        assert!(err.contains("gateway core context is too large"), "{err}");
        assert!(err.contains("MEMORY.md"), "{err}");
        assert!(err.contains(&format!("{expected_len} bytes")), "{err}");
        assert!(
            err.contains(&MAX_DEVELOPER_INSTRUCTIONS_BYTES.to_string()),
            "{err}"
        );
        assert!(
            err.contains("trim $XDG_CONFIG_HOME/gateway/MEMORY.md"),
            "{err}"
        );
    }

    #[test]
    fn ensure_heartbeat_prompt_file_returns_heartbeat_path() {
        let dir = tempdir().unwrap();
        let xdg_config_home = dir.path().join("config");

        let path = ensure_heartbeat_prompt_file(&xdg_config_home).unwrap();

        assert_eq!(path, xdg_config_home.join("gateway/HEARTBEAT.md"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            valid_template_header(HEARTBEAT_TEMPLATE)
        );
    }

    #[test]
    fn template_header_rejects_malformed_embedded_templates() {
        let err = template_header("AGENTS.md\n> **Scope:** broken heading\n").unwrap_err();
        assert!(err.contains("embedded context template"), "{err}");
        assert!(err.contains("first line"), "{err}");

        let err = template_header("# AGENTS.md\nScope: broken scope\n").unwrap_err();
        assert!(err.contains("embedded context template"), "{err}");
        assert!(err.contains("second line"), "{err}");
    }

    fn valid_template_header(template: &str) -> String {
        template_header(template).unwrap()
    }

    fn assert_in_order(text: &str, needles: &[&str]) {
        let mut offset = 0;
        for needle in needles {
            let found = text[offset..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle} after byte {offset}"));
            offset += found + needle.len();
        }
    }
}
