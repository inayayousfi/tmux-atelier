use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use glob::glob;

use crate::config::{quote_powershell, quote_sh, Config};
use crate::interaction::Interaction;
use crate::process;
use crate::workspace::{self, is_windows_shell};
use crate::{err, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub destination: String,
    pub path: String,
    pub shell: String,
}

pub fn choose_target(config: &Config, interaction: &dyn Interaction) -> Result<Option<Target>> {
    loop {
        let Some(machine) = choose_machine(config, interaction)? else {
            return Ok(None);
        };
        let mut columns = machine.split('\t');
        let machine_type = columns.next().unwrap_or_default();
        let mut destination = machine.rsplit('\t').next().unwrap_or_default().to_owned();
        config.debug(&format!(
            "new-workspace machine-type={machine_type} destination={destination}"
        ))?;
        let (shell, home) = match machine_type {
            "local" => (
                "local".to_owned(),
                env::var("HOME").map_err(|_| err("HOME is not configured"))?,
            ),
            "custom" => {
                let Some(input) =
                    interaction.input("OpenSSH destination (user@host or alias)", None)?
                else {
                    continue;
                };
                workspace::validate_destination(&input)?;
                destination = input;
                match remote_connection(config, &destination) {
                    Ok(connection) => connection,
                    Err(_) => {
                        eprintln!("Could not connect to {destination}.");
                        continue;
                    }
                }
            }
            "alias" => match remote_connection(config, &destination) {
                Ok(connection) => connection,
                Err(_) => {
                    eprintln!("Could not connect to {destination}.");
                    continue;
                }
            },
            _ => continue,
        };
        match choose_directory(config, interaction, &destination, &home, &shell)? {
            DirectoryChoice::Selected(path) => {
                return Ok(Some(Target {
                    destination,
                    path,
                    shell,
                }))
            }
            DirectoryChoice::Back => continue,
            DirectoryChoice::Cancelled => return Ok(None),
        }
    }
}

fn choose_machine(config: &Config, interaction: &dyn Interaction) -> Result<Option<String>> {
    let mut choices = vec![
        ("Local".to_owned(), "local\tLocal\tlocal".to_owned()),
        (
            "Custom SSH destination".to_owned(),
            "custom\tCustom SSH destination\t".to_owned(),
        ),
    ];
    let home = env::var_os("HOME").ok_or("HOME is not configured")?;
    let mut aliases = Vec::new();
    aliases_from(
        &PathBuf::from(home).join(".ssh/config"),
        &mut HashSet::new(),
        &mut aliases,
    )?;
    aliases.sort();
    aliases.dedup();
    for alias in aliases {
        if let Some(choice) = alias_choice(config, &alias)? {
            let label = choice
                .split_once('\t')
                .and_then(|(_, rest)| rest.rsplit_once('\t').map(|(label, _)| label))
                .unwrap_or(&alias)
                .to_owned();
            choices.push((label, choice));
        }
    }
    config.debug(&format!(
        "new-workspace machine-selection started rows={}",
        choices.len()
    ))?;
    let labels = choices
        .iter()
        .map(|(label, _)| label.clone())
        .collect::<Vec<_>>();
    let result = interaction
        .choose("Machine", &labels)?
        .map(|index| choices[index].1.clone());
    if let Some(choice) = &result {
        config.debug(&format!(
            "new-workspace machine-selection selected={}",
            crate::config::shell_debug(choice)
        ))?;
    } else {
        config.debug("new-workspace machine-selection cancelled")?;
    }
    Ok(result)
}

fn aliases_from(file: &Path, seen: &mut HashSet<PathBuf>, aliases: &mut Vec<String>) -> Result<()> {
    if !file.is_file() {
        return Ok(());
    }
    let canonical = fs::canonicalize(file)?;
    if !seen.insert(canonical.clone()) {
        return Ok(());
    }
    for raw in fs::read_to_string(&canonical)?.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        let mut parts = line.splitn(2, char::is_whitespace);
        let keyword = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default().trim();
        if keyword.eq_ignore_ascii_case("host") {
            aliases.extend(
                rest.split_whitespace()
                    .filter(|pattern| {
                        !pattern.starts_with('!') && !pattern.contains(['*', '?', '!'])
                    })
                    .map(str::to_owned),
            );
        } else if keyword.eq_ignore_ascii_case("include") {
            for include in rest.split_whitespace() {
                let expanded = if let Some(suffix) = include.strip_prefix('~') {
                    PathBuf::from(env::var_os("HOME").ok_or("HOME is not configured")?)
                        .join(suffix.trim_start_matches('/'))
                } else if Path::new(include).is_absolute() {
                    PathBuf::from(include)
                } else {
                    canonical.parent().unwrap_or(Path::new("/")).join(include)
                };
                let pattern = expanded.to_string_lossy();
                for entry in glob(&pattern)? {
                    let entry = entry?;
                    if entry != canonical {
                        aliases_from(&entry, seen, aliases)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn alias_choice(config: &Config, alias: &str) -> Result<Option<String>> {
    let Some(output) = process::output_quiet("ssh", ["-G", "--", alias]) else {
        return Ok(None);
    };
    let mut user = "";
    let mut hostname = "";
    let mut port = "";
    for line in output.lines() {
        if let Some((key, value)) = line.split_once(char::is_whitespace) {
            match key {
                "user" if user.is_empty() => user = value.trim(),
                "hostname" if hostname.is_empty() => hostname = value.trim(),
                "port" if port.is_empty() => port = value.trim(),
                _ => {}
            }
        }
    }
    if user.is_empty() || hostname.is_empty() {
        return Ok(None);
    }
    let port = if port.is_empty() { "22" } else { port };
    config.debug(&format!(
        "new-workspace ssh-alias alias={alias} resolved={user}@{hostname}:{port}"
    ))?;
    Ok(Some(format!(
        "alias\t{alias}  {user}@{hostname}:{port}\t{alias}"
    )))
}

fn remote_connection(config: &Config, destination: &str) -> Result<(String, String)> {
    let direct = "[Console]::Out.WriteLine([Environment]::GetFolderPath('UserProfile'))";
    if let Ok(home) = process::ssh_output(config, destination, direct, true) {
        if !home.is_empty() {
            return Ok(("powershell".into(), home.trim_end_matches('\r').into()));
        }
    }
    let cmd = "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"[Console]::Out.WriteLine([Environment]::GetFolderPath('UserProfile'))\"";
    if let Ok(home) = process::ssh_output(config, destination, cmd, true) {
        if !home.is_empty() {
            return Ok(("windows".into(), home.trim_end_matches('\r').into()));
        }
    }
    let home = process::ssh_output(config, destination, "printf \"%s\\n\" \"$HOME\"", false)?;
    if home.is_empty() {
        Err(err("remote home is empty"))
    } else {
        Ok(("posix".into(), home))
    }
}

enum DirectoryChoice {
    Selected(String),
    Back,
    Cancelled,
}

fn choose_directory(
    config: &Config,
    interaction: &dyn Interaction,
    destination: &str,
    home: &str,
    shell: &str,
) -> Result<DirectoryChoice> {
    let mut current = home.to_owned();
    loop {
        let directories = if destination == "local" {
            local_directories(&current)?
        } else {
            match remote_directories(config, destination, &current, shell) {
                Ok(entries) => entries,
                Err(error) => {
                    eprintln!("Could not list remote directory: {current}");
                    return Err(error);
                }
            }
        };
        let prompt = display_path(home, &current, shell);
        let mut choices = vec![
            ("< Select this folder".to_owned(), "select".to_owned()),
            ("< Enter a path".to_owned(), "custom".to_owned()),
            ("< Back".to_owned(), "back".to_owned()),
        ];
        if current != "/" && !(is_windows_shell(shell) && is_windows_root(&current)) {
            choices.push(("< Up".to_owned(), "up".to_owned()));
        }
        for name in directories {
            choices.push((format!("{name}/"), format!("directory\t{name}")));
        }
        config.debug(&format!(
            "new-workspace directory-selection started destination={destination} current={} rows={}",
            crate::config::shell_debug(&current),
            choices.len()
        ))?;
        let labels = choices
            .iter()
            .map(|(label, _)| label.clone())
            .collect::<Vec<_>>();
        let Some(index) = interaction.choose(&prompt, &labels)? else {
            return Ok(DirectoryChoice::Cancelled);
        };
        let choice = &choices[index].1;
        config.debug(&format!("new-workspace directory-selection selected destination={destination} current={} choice={}", crate::config::shell_debug(&current), crate::config::shell_debug(choice)))?;
        if choice == "custom" {
            let Some(query) = interaction.input("Path", Some(&current))? else {
                continue;
            };
            let candidate = resolve_custom_path(home, &query, shell);
            let path = if destination == "local" {
                fs::create_dir_all(&candidate)?;
                workspace::canonical_local_path(&candidate)?
            } else {
                create_remote_path(config, destination, &candidate, shell)?
            };
            return Ok(DirectoryChoice::Selected(path));
        }
        let (kind, name) = choice.split_once('\t').unwrap_or((choice, ""));
        match kind {
            "select" => return Ok(DirectoryChoice::Selected(current)),
            "back" => return Ok(DirectoryChoice::Back),
            "up" => current = parent_path(&current, shell),
            "directory" => {
                current = if is_windows_shell(shell) {
                    format!("{}\\{name}", current.trim_end_matches(['/', '\\']))
                } else if current == "/" {
                    format!("/{name}")
                } else {
                    format!("{current}/{name}")
                };
            }
            _ => {}
        }
    }
}

fn local_directories(path: &str) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && !name.contains('\n') {
            entries.push(name);
        }
    }
    entries.sort();
    Ok(entries)
}

fn remote_directories(
    config: &Config,
    destination: &str,
    path: &str,
    shell: &str,
) -> Result<Vec<String>> {
    let remote = if is_windows_shell(shell) {
        format!("powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"Get-ChildItem -LiteralPath {} -Force -Directory | ForEach-Object {{ [Console]::Out.WriteLine($_.Name) }}\"", quote_powershell(path))
    } else {
        let inner = "cd -- \"$1\" 2>/dev/null || exit 1\nfor entry in ./* ./.[!.]* ./..?*; do\n    [ -d \"$entry\" ] || continue\n    name=${entry#./}\n    case $name in *\"\n\"*) continue ;; esac\n    printf \"%s\\n\" \"$name\"\ndone";
        format!("sh -c {} sh {}", quote_sh(inner), quote_sh(path))
    };
    let output = process::ssh_output(config, destination, &remote, false)?;
    let mut entries: Vec<_> = output
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    if !is_windows_shell(shell) {
        entries.sort();
        entries.dedup();
    }
    Ok(entries)
}

fn create_remote_path(
    config: &Config,
    destination: &str,
    path: &str,
    shell: &str,
) -> Result<String> {
    let remote = if is_windows_shell(shell) {
        format!("powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"New-Item -ItemType Directory -Force -LiteralPath {} | Out-Null; [Console]::Out.WriteLine((Resolve-Path -LiteralPath {}).Path)\"", quote_powershell(path), quote_powershell(path))
    } else {
        let inner = r#"mkdir -p -- "$1" && cd -- "$1" && pwd -P"#;
        format!("sh -c {} sh {}", quote_sh(inner), quote_sh(path))
    };
    Ok(process::ssh_output(config, destination, &remote, false)?
        .trim_end_matches('\r')
        .into())
}

pub fn resolve_custom_path(home: &str, input: &str, shell: &str) -> String {
    if is_windows_shell(shell) {
        if input == "~" {
            home.into()
        } else if let Some(rest) = input
            .strip_prefix("~/")
            .or_else(|| input.strip_prefix("~\\"))
        {
            format!("{}\\{rest}", home.trim_end_matches(['/', '\\']))
        } else if is_windows_absolute(input) {
            input.into()
        } else {
            format!("{}\\{input}", home.trim_end_matches(['/', '\\']))
        }
    } else if input == "~" {
        home.into()
    } else if let Some(rest) = input.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else if input.starts_with('/') {
        input.into()
    } else {
        format!("{}/{input}", home.trim_end_matches('/'))
    }
}

fn parent_path(path: &str, shell: &str) -> String {
    if is_windows_shell(shell) {
        let trimmed = path.trim_end_matches(['/', '\\']);
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            return format!("{trimmed}\\");
        }
        let parent = trimmed
            .rsplit_once(['/', '\\'])
            .map(|(parent, _)| parent)
            .unwrap_or(trimmed);
        if parent.len() == 2 && parent.ends_with(':') {
            format!("{parent}\\")
        } else {
            parent.into()
        }
    } else if path == "/" {
        "/".into()
    } else {
        Path::new(path.trim_end_matches('/'))
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .into_owned()
    }
}

fn display_path(home: &str, path: &str, shell: &str) -> String {
    if path == home {
        "~/".into()
    } else if is_windows_shell(shell)
        && path.starts_with(home)
        && path[home.len()..].starts_with(['/', '\\'])
    {
        format!("~\\{}", &path[home.len() + 1..])
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.into()
    }
}

fn is_windows_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || path.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_paths_resolve_for_both_shell_families() {
        assert_eq!(
            resolve_custom_path("/home/me", "~/work", "posix"),
            "/home/me/work"
        );
        assert_eq!(
            resolve_custom_path(r"C:\Users\me", r"~\work", "windows"),
            r"C:\Users\me\work"
        );
        assert_eq!(
            resolve_custom_path(r"C:\Users\me", r"D:\work", "windows"),
            r"D:\work"
        );
    }
}
