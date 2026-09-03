use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::process;
use crate::{Result, err};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub destination: String,
    pub path: String,
    pub created: String,
    pub shell: String,
}

impl Workspace {
    pub fn new(name: &str, destination: &str, path: &str, shell: Option<&str>) -> Result<Self> {
        validate_name(name)?;
        validate_destination(destination)?;
        validate_value("path", path)?;
        let shell = shell.unwrap_or(if destination == "local" {
            "local"
        } else {
            "posix"
        });
        validate_shell(destination, shell)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        Ok(Self {
            name: name.into(),
            destination: destination.into(),
            path: path.into(),
            created: format!("{}.{:09}", now.as_secs(), now.subsec_nanos()),
            shell: shell.into(),
        })
    }
}

pub fn validate_name(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid
        .then_some(())
        .ok_or_else(|| err(format!("invalid workspace name: {value}")))
}

pub fn validate_value(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        Err(err(format!("{label} must not be empty")))
    } else if value.contains(['\n', '\r']) {
        Err(err(format!("{label} must not contain a newline")))
    } else {
        Ok(())
    }
}

pub fn validate_destination(value: &str) -> Result<()> {
    validate_value("destination", value)?;
    let valid = value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'@' | b'-'));
    if value == "local" || valid {
        Ok(())
    } else {
        Err(err(format!("invalid OpenSSH destination: {value}")))
    }
}

pub fn is_windows_shell(shell: &str) -> bool {
    matches!(shell, "windows" | "powershell")
}

fn valid_created(value: &str) -> bool {
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    !seconds.is_empty()
        && seconds.bytes().all(|byte| byte.is_ascii_digit())
        && (fraction.is_empty() && !value.contains('.')
            || !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()))
}

fn validate_shell(destination: &str, shell: &str) -> Result<()> {
    let valid = if destination == "local" {
        shell == "local"
    } else {
        shell == "posix" || is_windows_shell(shell)
    };
    valid.then_some(()).ok_or_else(|| {
        err(format!(
            "invalid shell for {} workspace: {shell}",
            if destination == "local" {
                "local"
            } else {
                "remote"
            }
        ))
    })
}

pub fn path(config: &Config, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(config.workspaces.join(name))
}

pub fn read(config: &Config, name: &str) -> Result<Workspace> {
    let file = path(config, name)?;
    let contents = fs::read_to_string(&file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            err(format!("workspace not found: {name}"))
        } else {
            error.into()
        }
    })?;
    let mut values = std::collections::HashMap::new();
    for line in contents.lines() {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        if matches!(key, "name" | "destination" | "path" | "created" | "shell")
            && values.insert(key, value).is_some()
        {
            let label = match key {
                "created" => "creation time",
                other => other,
            };
            return Err(err(format!("duplicate {label} in workspace: {name}")));
        }
    }
    let stored_name = values
        .get("name")
        .ok_or_else(|| err(format!("incomplete workspace: {name}")))?;
    let destination = values
        .get("destination")
        .ok_or_else(|| err(format!("incomplete workspace: {name}")))?;
    let target = values
        .get("path")
        .ok_or_else(|| err(format!("incomplete workspace: {name}")))?;
    if stored_name != &name {
        return Err(err(format!(
            "workspace name does not match its filename: {name}"
        )));
    }
    validate_name(stored_name)?;
    validate_destination(destination)?;
    validate_value("path", target)?;
    let shell = values
        .get("shell")
        .copied()
        .unwrap_or(if *destination == "local" {
            "local"
        } else {
            "posix"
        });
    validate_shell(destination, shell)
        .map_err(|_| err(format!("invalid shell in workspace: {name}")))?;
    let created = if let Some(created) = values.get("created") {
        if !valid_created(created) {
            return Err(err(format!("invalid creation time in workspace: {name}")));
        }
        (*created).to_owned()
    } else {
        fs::metadata(file)?.mtime().to_string()
    };
    Ok(Workspace {
        name: name.into(),
        destination: (*destination).into(),
        path: (*target).into(),
        created,
        shell: shell.into(),
    })
}

pub fn write(config: &Config, workspace: &Workspace, create: bool) -> Result<()> {
    validate_name(&workspace.name)?;
    validate_destination(&workspace.destination)?;
    validate_value("path", &workspace.path)?;
    validate_shell(&workspace.destination, &workspace.shell)?;
    if !valid_created(&workspace.created) {
        return Err(err(format!("invalid creation time: {}", workspace.created)));
    }
    config.secure_dir(&config.workspaces)?;
    let file = path(config, &workspace.name)?;
    let temporary = tempfile_path(&config.workspaces, &workspace.name)?;
    let result = (|| -> Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        write!(
            output,
            "name={}\ndestination={}\npath={}\ncreated={}\nshell={}\n",
            workspace.name,
            workspace.destination,
            workspace.path,
            workspace.created,
            workspace.shell
        )?;
        output.sync_all()?;
        if create {
            fs::hard_link(&temporary, &file)?;
            fs::remove_file(&temporary)?;
        } else {
            fs::rename(&temporary, &file)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn tempfile_path(directory: &Path, name: &str) -> Result<PathBuf> {
    for sequence in 0..1000u32 {
        let candidate = directory.join(format!(".{name}.{}.{sequence}", std::process::id()));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(err("could not allocate temporary workspace file"))
}

pub fn definition_names(config: &Config) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = match fs::read_dir(&config.workspaces) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && !name.starts_with('.') {
            names.push(name);
        }
    }
    Ok(names)
}

pub fn parse_target(target: &str) -> Result<(String, String)> {
    let (destination, path) = target
        .split_once(':')
        .ok_or_else(|| err("target must use destination:path"))?;
    validate_destination(destination)?;
    validate_value("path", path)?;
    Ok((destination.into(), path.into()))
}

pub fn session_exists(config: &Config, name: &str) -> bool {
    process::tmux_success(config, &["has-session", "-t", &format!("={name}")])
}

pub fn session_option(config: &Config, name: &str, option: &str) -> String {
    process::tmux_quiet(
        config,
        &["show-options", "-qv", "-t", &format!("={name}:"), option],
    )
    .unwrap_or_default()
}

pub fn session_names(config: &Config) -> Vec<String> {
    process::tmux_quiet(config, &["list-sessions", "-F", "#{session_name}"])
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

pub fn normalise_name(
    config: &Config,
    target: &str,
    excluded_session: &str,
    excluded_definition: &str,
) -> String {
    let trimmed = target.trim_end_matches('/');
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty() && !matches!(*name, "." | "~"))
        .unwrap_or("workspace");
    let mut normalized = String::new();
    for character in base.chars() {
        let character = if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            character
        } else {
            '-'
        };
        normalized.push(character);
    }
    let normalized = normalized.trim_matches('-');
    let base = if normalized.is_empty() {
        "workspace"
    } else {
        normalized
    };
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    loop {
        let definition_conflict =
            candidate != excluded_definition && config.workspaces.join(&candidate).exists();
        let session_conflict = candidate != excluded_session && session_exists(config, &candidate);
        if !definition_conflict && !session_conflict {
            return candidate;
        }
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
}

pub fn canonical_local_path(path: &str) -> Result<String> {
    Ok(fs::canonicalize(path)?.to_string_lossy().into_owned())
}

pub fn workspace_for_local_path(config: &Config, wanted: &str) -> Result<Option<String>> {
    for name in definition_names(config)? {
        let workspace = read(config, &name)?;
        if workspace.destination == "local"
            && canonical_local_path(&workspace.path).ok().as_deref() == Some(wanted)
        {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

pub fn all_names(config: &Config) -> Result<Vec<String>> {
    crate::snapshot::lock(config, &config.workspace_order_lock, || {
        ordered_names(config)
    })
}

fn creation_order(config: &Config) -> Result<Vec<String>> {
    let definitions = definition_names(config)?;
    let definition_set: HashSet<_> = definitions.iter().cloned().collect();
    let mut ordered: Vec<(f64, String)> = definitions
        .into_iter()
        .map(|name| {
            read(config, &name).map(|workspace| (workspace.created.parse().unwrap_or(0.0), name))
        })
        .collect::<Result<_>>()?;
    if let Some(sessions) = process::tmux_quiet(
        config,
        &["list-sessions", "-F", "#{session_created}\t#{session_name}"],
    ) {
        for line in sessions.lines() {
            if let Some((created, name)) = line.split_once('\t')
                && !definition_set.contains(name)
            {
                ordered.push((created.parse().unwrap_or(0.0), name.into()));
            }
        }
    }
    ordered.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    Ok(ordered.into_iter().map(|(_, name)| name).collect())
}

fn stored_order(config: &Config) -> Result<Vec<String>> {
    let contents = match fs::read_to_string(&config.workspace_order_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for name in contents.lines() {
        validate_name(name).map_err(|_| err("invalid workspace order"))?;
        if !seen.insert(name.to_owned()) {
            return Err(err("invalid workspace order"));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn ordered_names(config: &Config) -> Result<Vec<String>> {
    let created = creation_order(config)?;
    let mut available: HashSet<_> = created.iter().cloned().collect();
    let mut ordered = Vec::with_capacity(created.len());
    for name in stored_order(config)? {
        if available.remove(&name) {
            ordered.push(name);
        }
    }
    ordered.extend(created.into_iter().filter(|name| available.contains(name)));
    Ok(ordered)
}

fn write_order(config: &Config, names: &[String]) -> Result<()> {
    config.secure_dir(&config.state_root)?;
    let temporary = tempfile_path(&config.state_root, "workspace.order")?;
    let result = (|| -> Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        for name in names {
            validate_name(name)?;
            writeln!(output, "{name}")?;
        }
        output.sync_all()?;
        fs::rename(&temporary, &config.workspace_order_file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn reorder(config: &Config, source: &str, target: &str) -> Result<()> {
    validate_name(source)?;
    validate_name(target)?;
    crate::snapshot::lock(config, &config.workspace_order_lock, || {
        let mut names = ordered_names(config)?;
        let Some(source_index) = names.iter().position(|name| name == source) else {
            return Ok(());
        };
        let Some(target_index) = names.iter().position(|name| name == target) else {
            return Ok(());
        };
        if source_index == target_index {
            return Ok(());
        }
        let source = names.remove(source_index);
        names.insert(target_index, source);
        write_order(config, &names)
    })
}

pub fn update_order(
    config: &Config,
    operation: impl FnOnce(&mut Vec<String>) -> Result<()>,
) -> Result<()> {
    crate::snapshot::lock(config, &config.workspace_order_lock, || {
        let mut names = ordered_names(config)?;
        operation(&mut names)?;
        write_order(config, &names)
    })
}

pub fn create_session(config: &Config, workspace: &Workspace, initial_path: &str) -> Result<()> {
    if workspace.destination == "local" {
        if !Path::new(&workspace.path).is_dir() {
            return Err(err(format!(
                "local path is not a directory: {}",
                workspace.path
            )));
        }
        let initial = if Path::new(initial_path).is_dir() {
            initial_path
        } else {
            &workspace.path
        };
        process::tmux(
            config,
            &["new-session", "-d", "-s", &workspace.name, "-c", initial],
        )?;
    } else {
        let command = process::remote_shell_command(
            config,
            &workspace.destination,
            &workspace.path,
            &workspace.shell,
        )?;
        process::tmux(
            config,
            &["new-session", "-d", "-s", &workspace.name, &command],
        )?;
    }
    if let Err(error) = mark_session(config, workspace) {
        let _ = process::tmux(
            config,
            &["kill-session", "-t", &format!("={}", workspace.name)],
        );
        return Err(error);
    }
    Ok(())
}

pub fn create_restore_session(
    config: &Config,
    workspace: &Workspace,
    initial_path: &str,
    generation: &str,
) -> Result<()> {
    let mut arguments = if workspace.destination == "local" {
        if !Path::new(&workspace.path).is_dir() {
            return Err(err(format!(
                "local path is not a directory: {}",
                workspace.path
            )));
        }
        let initial = if Path::new(initial_path).is_dir() {
            initial_path
        } else {
            &workspace.path
        };
        vec![
            "new-session".to_owned(),
            "-d".into(),
            "-s".into(),
            workspace.name.clone(),
            "-c".into(),
            initial.into(),
        ]
    } else {
        vec![
            "new-session".to_owned(),
            "-d".into(),
            "-s".into(),
            workspace.name.clone(),
            process::remote_shell_command(
                config,
                &workspace.destination,
                &workspace.path,
                &workspace.shell,
            )?,
        ]
    };
    arguments.extend([
        ";".into(),
        "set-option".into(),
        "-q".into(),
        "-t".into(),
        format!("={}:", workspace.name),
        "@atelier_restore_owner".into(),
        generation.into(),
    ]);
    process::tmux(
        config,
        &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    if let Err(error) = mark_session(config, workspace) {
        let _ = process::tmux(
            config,
            &["kill-session", "-t", &format!("={}", workspace.name)],
        );
        return Err(error);
    }
    Ok(())
}

pub fn mark_session(config: &Config, workspace: &Workspace) -> Result<()> {
    let target = format!("={}:", workspace.name);
    for (option, value) in [
        ("@atelier_managed", "1"),
        ("@atelier_destination", workspace.destination.as_str()),
        ("@atelier_path", workspace.path.as_str()),
        ("@atelier_shell", workspace.shell.as_str()),
    ] {
        process::tmux(config, &["set-option", "-q", "-t", &target, option, value])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn config(root: &Path) -> Config {
        Config {
            tmux: "false".into(),
            state_root: root.into(),
            workspaces: root.join("workspaces"),
            debug_log: root.join("debug.log"),
            restore_file: root.join("restore.snapshot"),
            workspace_order_file: root.join("workspace.order"),
            ssh_destinations_file: root.join("ssh-destinations"),
            snapshot_lock: root.join(".snapshot.lock"),
            workspace_order_lock: root.join(".workspace-order.lock"),
            status_lock: root.join(".status.lock"),
            adoption_lock: root.join(".adoption.lock"),
            restore_lock: root.join(".restore.lock"),
        }
    }

    #[test]
    fn workspace_round_trip_preserves_equals_and_spaces() {
        let temporary = tempfile::tempdir().unwrap();
        let config = config(temporary.path());
        let workspace = Workspace::new("example", "local", "/tmp/a path=x", None).unwrap();
        write(&config, &workspace, true).unwrap();
        assert_eq!(read(&config, "example").unwrap(), workspace);
        assert_eq!(
            fs::metadata(path(&config, "example").unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn target_uses_only_first_colon() {
        assert_eq!(
            parse_target("host:C:\\work").unwrap(),
            ("host".into(), "C:\\work".into())
        );
    }

    #[test]
    fn creation_time_requires_plain_decimal_digits() {
        assert!(valid_created("123"));
        assert!(valid_created("123.456"));
        assert!(!valid_created("1e3"));
        assert!(!valid_created("NaN"));
        assert!(!valid_created("123."));
    }

    #[test]
    fn normalized_names_match_shell_replacement_behavior() {
        let temporary = tempfile::tempdir().unwrap();
        let config = config(temporary.path());
        assert_eq!(normalise_name(&config, "/tmp/a..b", "", ""), "a--b");
    }

    #[test]
    fn custom_order_migrates_from_creation_order_and_supports_mutation() {
        let temporary = tempfile::tempdir().unwrap();
        let config = config(temporary.path());
        for (name, created) in [("one", "1"), ("two", "2"), ("three", "3")] {
            let mut workspace = Workspace::new(name, "local", "/tmp", None).unwrap();
            workspace.created = created.into();
            write(&config, &workspace, true).unwrap();
        }

        assert_eq!(all_names(&config).unwrap(), ["one", "two", "three"]);
        assert!(!config.workspace_order_file.exists());

        reorder(&config, "one", "three").unwrap();
        assert_eq!(all_names(&config).unwrap(), ["two", "three", "one"]);
        assert_eq!(
            fs::read_to_string(&config.workspace_order_file).unwrap(),
            "two\nthree\none\n"
        );
        assert_eq!(
            fs::metadata(&config.workspace_order_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        update_order(&config, |names| {
            let index = names.iter().position(|name| name == "three").unwrap();
            names[index] = "renamed".into();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&config.workspace_order_file).unwrap(),
            "two\nrenamed\none\n"
        );
    }

    #[test]
    fn concurrent_reorders_keep_a_complete_valid_order() {
        let temporary = tempfile::tempdir().unwrap();
        let config = config(temporary.path());
        for (name, created) in [("one", "1"), ("two", "2"), ("three", "3")] {
            let mut workspace = Workspace::new(name, "local", "/tmp", None).unwrap();
            workspace.created = created.into();
            write(&config, &workspace, true).unwrap();
        }
        let first = config.clone();
        let second = config.clone();
        let one = std::thread::spawn(move || reorder(&first, "one", "three"));
        let two = std::thread::spawn(move || reorder(&second, "three", "one"));
        one.join().unwrap().unwrap();
        two.join().unwrap().unwrap();

        let names = all_names(&config).unwrap();
        assert_eq!(names.len(), 3);
        assert!(names.iter().any(|name| name == "one"));
        assert!(names.iter().any(|name| name == "two"));
        assert!(names.iter().any(|name| name == "three"));
        assert_eq!(stored_order(&config).unwrap(), names);
    }
}
