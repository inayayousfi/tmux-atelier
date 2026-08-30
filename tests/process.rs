use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct TestEnv {
    root: TempDir,
    repo: PathBuf,
    cli: PathBuf,
    tmux: PathBuf,
    real_tmux: PathBuf,
    socket: String,
    state: PathBuf,
    ssh_log: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let socket = format!(
            "atelier-rust-{}",
            root.path().file_name().unwrap().to_string_lossy()
        );
        let real_tmux = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|directory| directory.join("tmux"))
            .find(|path| path.is_file())
            .expect("tmux is required for process tests");
        Self {
            state: root.path().join("state"),
            ssh_log: root.path().join("ssh.log"),
            tmux: repo.join("tests/fixtures/tmux"),
            real_tmux,
            cli: PathBuf::from(env!("CARGO_BIN_EXE_tmux-atelier")),
            repo,
            root,
            socket,
        }
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        let fixtures = self.repo.join("tests/fixtures");
        command
            .env("TMUX_ATELIER_TEST_SOCKET", &self.socket)
            .env("TMUX_ATELIER_REAL_TMUX", &self.real_tmux)
            .env("TMUX_ATELIER_STATE_DIR", &self.state)
            .env("TMUX_ATELIER_SSH_LOG", &self.ssh_log)
            .env("TMUX_ATELIER_TMUX", &self.tmux)
            .env("TMUX_ATELIER_CLI", &self.cli)
            .env("HOME", self.root.path())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    fixtures.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            );
        command
    }

    fn cli<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command(&self.cli).args(args).output().unwrap()
    }

    fn ok<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        assert!(self
            .command(&self.cli)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }

    fn tmux<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command(&self.tmux).args(args).output().unwrap()
    }

    fn tmux_ok<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        assert!(self
            .command(&self.tmux)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }

    fn tmux_text<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.tmux(args);
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
    }

    fn workspace(&self, name: &str) -> PathBuf {
        self.state.join("workspaces").join(name)
    }

    fn scripted(&self, responses: &str) -> PathBuf {
        let path = self.root.path().join("interaction");
        fs::write(&path, responses).unwrap();
        path
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = self.tmux(["kill-server"]);
    }
}

#[test]
fn local_lifecycle_tabs_and_concurrent_creation() {
    let env = TestEnv::new();
    let first = env.root.path().join("local one=ok");
    let second = env.root.path().join("local two");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();

    env.ok([
        "new",
        &format!("local:{}", first.display()),
        "local-one",
        "--detached",
    ]);
    let definition = fs::read_to_string(env.workspace("local-one")).unwrap();
    assert!(definition.contains(&format!("path={}", first.display())));
    assert_eq!(
        fs::metadata(env.workspace("local-one"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=local-one:",
            "#{pane_current_path}",
        ]),
        first.to_string_lossy()
    );

    env.ok(["window", "local-one"]);
    let pane = env.tmux_text(["display-message", "-p", "-t", "=local-one:1", "#{pane_id}"]);
    env.ok(["split", "vertical", &pane]);
    assert_eq!(
        env.tmux_text(["list-panes", "-t", "=local-one:1", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );

    env.ok(["edit", "local-one", &format!("local:{}", second.display())]);
    env.ok(["close", "local-one"]);
    assert!(env.workspace("local-one").is_file());
    env.ok(["open", "local-one", "--detached"]);
    env.ok(["rename", "local-one", "renamed"]);
    assert!(env.workspace("renamed").is_file());
    assert!(!env.workspace("local-one").exists());

    let target = format!("local:{}", first.display());
    let mut one = env.command(&env.cli);
    one.args(["new", &target, "race", "--detached"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut two = env.command(&env.cli);
    two.args(["new", &target, "race", "--detached"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut one = one.spawn().unwrap();
    let mut two = two.spawn().unwrap();
    let first_status = one.wait().unwrap().success();
    let second_status = two.wait().unwrap().success();
    assert_ne!(first_status, second_status);
    assert!(env.workspace("race").is_file());

    assert!(!env.cli(["close", "renamed", "unexpected"]).status.success());
    env.ok(["delete", "renamed"]);
    assert!(!env.workspace("renamed").exists());

    env.ok(["new", &target, "-leading", "--detached"]);
    assert!(env.workspace("-leading").is_file());
    env.ok(["delete", "-leading"]);
}

#[test]
fn remote_windows_and_splits_use_shared_ssh() {
    let env = TestEnv::new();
    let path = "/srv/app dir/it's";
    env.ok(["new", &format!("deploy@app:{path}"), "remote", "--detached"]);
    env.ok(["window", "remote"]);
    let pane = env.tmux_text(["display-message", "-p", "-t", "=remote:1", "#{pane_id}"]);
    env.ok(["split", "horizontal", &pane]);

    let bytes = fs::read(&env.ssh_log).unwrap();
    let arguments = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(arguments.len(), 27);
    assert_eq!(
        &arguments[..4],
        ["-o", "ControlMaster=auto", "-o", "ControlPersist=60"]
    );
    assert!(arguments[5].starts_with("ControlPath="));
    assert_eq!(arguments[6], "-t");
    assert_eq!(arguments[7], "deploy@app");
    assert!(arguments[8].contains("/srv/app dir/it'\\''s"));
}

#[test]
fn plugin_adapter_configures_options_bindings_and_hooks() {
    let env = TestEnv::new();
    env.tmux_ok(["new-session", "-d", "-s", "native"]);
    env.tmux_ok(["set-option", "-g", "prefix", "C-Space"]);
    env.tmux_ok(["set-option", "-g", "status-position", "top"]);
    env.tmux_ok(["set-option", "-g", "@atelier_tab_separator", "::"]);
    env.tmux_ok(["bind-key", "x", "display-message", "user-binding"]);
    env.tmux_ok(["bind-key", "y", "display-message", "another window action"]);

    let output = env
        .command(&env.repo.join("tmux-atelier.tmux"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(env.tmux_text(["show-options", "-gv", "prefix"]), "C-Space");
    assert_eq!(
        env.tmux_text(["show-options", "-gv", "status-position"]),
        "top"
    );
    assert_eq!(env.tmux_text(["show-options", "-gv", "mouse"]), "on");
    assert_eq!(env.tmux_text(["show-options", "-gv", "status"]), "2");
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore"]),
        "prompt"
    );
    let status = env.tmux_text(["show-options", "-gqv", "@atelier_tabs_format"]);
    assert!(status.contains("::"));
    assert!(status.contains("range=window"));
    let keys = env.tmux_text(["list-keys", "-T", "prefix"]);
    assert!(keys.contains("user-binding"));
    assert!(keys.contains("another window action"));
    assert!(keys.contains(" window \\\"#{session_name}\\\""), "{keys}");
    let hooks = env.tmux_text(["show-hooks", "-g"]);
    assert!(hooks.contains("internal refresh-status"));
    assert!(hooks.contains("internal snapshot"));
    assert!(hooks.contains("internal restore-start"));
    assert!(hooks.contains("internal adopt-session"));
}

#[test]
fn scripted_wizard_edits_and_renames_workspace() {
    let env = TestEnv::new();
    let first = env.root.path().join("first");
    let second = env.root.path().join("new nested target");
    fs::create_dir_all(&first).unwrap();
    env.ok([
        "new",
        &format!("local:{}", first.display()),
        "before",
        "--detached",
    ]);
    let responses = env.scripted(&format!(
        "choose\tLocal\nchoose\t[ Enter a path ]\ninput\t{}\ninput\tafter\n",
        second.display()
    ));
    let log = env.root.path().join("interaction.log");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", &responses)
        .env("TMUX_ATELIER_INTERACTION_LOG", &log)
        .args(["internal", "popup-edit", "before"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(second.is_dir());
    let definition = fs::read_to_string(env.workspace("after")).unwrap();
    assert!(definition.contains(&format!("path={}", second.display())));
    assert!(!env.workspace("before").exists());
    let interactions = fs::read_to_string(log).unwrap();
    assert!(interactions.contains("choose\tMachine\tLocal\tCustom SSH destination"));
    assert!(interactions.contains("[ Enter a path ]"));

    let collision = env.root.path().join("< Back");
    fs::create_dir(&collision).unwrap();
    let responses = env.scripted(
        "choose\tLocal\nchoose\t< Back/\nchoose\t[ Select this folder ]\ninput\tcollision\n",
    );
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", responses)
        .args(["internal", "popup-edit", "after"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(fs::read_to_string(env.workspace("collision"))
        .unwrap()
        .contains(&format!("path={}", collision.display())));
}

#[test]
fn wizard_recovers_and_remembers_ssh_destinations() {
    let env = TestEnv::new();
    let local = env.root.path().join("local");
    fs::create_dir(&local).unwrap();
    fs::write(
        env.root.path().join(".bash_history"),
        "ssh -p 2200 history@winhost\n",
    )
    .unwrap();
    env.ok([
        "new",
        &format!("local:{}", local.display()),
        "before",
        "--detached",
    ]);

    let history = env
        .scripted("choose\thistory@winhost\nchoose\t[ Select this folder ]\ninput\tfrom-history\n");
    let history_log = env.root.path().join("history.log");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", history)
        .env("TMUX_ATELIER_INTERACTION_LOG", &history_log)
        .args(["internal", "popup-edit", "before"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(env.workspace("from-history"))
        .unwrap()
        .contains("destination=history@winhost"));
    assert!(fs::read_to_string(history_log)
        .unwrap()
        .contains("Modèles/"));

    let custom = env.scripted(
        "choose\tCustom SSH destination\ninput\tremembered@app.example\nchoose\t[ Select this folder ]\ninput\tremembered\n",
    );
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", custom)
        .args(["internal", "popup-edit", "from-history"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let destinations = env.state.join("ssh-destinations");
    assert_eq!(
        fs::read_to_string(&destinations).unwrap(),
        "history@winhost\nremembered@app.example\n"
    );
    assert_eq!(
        fs::metadata(&destinations).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let cancel = env.scripted("cancel\n");
    let log = env.root.path().join("remembered.log");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", cancel)
        .env("TMUX_ATELIER_INTERACTION_LOG", &log)
        .args(["internal", "popup-edit", "remembered"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let choices = fs::read_to_string(log).unwrap();
    assert!(choices.contains("history@winhost"));
    assert!(choices.contains("remembered@app.example"));
}

#[test]
fn scripted_confirmation_controls_destructive_actions() {
    let env = TestEnv::new();
    let path = env.root.path().join("project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "project",
        "--detached",
    ]);

    let reject = env.scripted("confirm\tfalse\n");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", reject)
        .args(["internal", "confirm-delete", "project"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(env.workspace("project").is_file());

    let accept = env.scripted("confirm\ttrue\n");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", accept)
        .args(["internal", "confirm-delete", "project"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!env.workspace("project").exists());
    assert!(!env.tmux(["has-session", "-t", "=project"]).status.success());
}

#[test]
fn native_session_is_adopted_as_a_workspace() {
    let env = TestEnv::new();
    let path = env.root.path().join("native adopted");
    fs::create_dir(&path).unwrap();
    env.tmux_ok([
        "new-session",
        "-d",
        "-s",
        "native",
        "-c",
        path.to_str().unwrap(),
    ]);
    env.ok(["internal", "adopt-session", "native"]);
    assert!(env.workspace("native-adopted").is_file());
    assert_eq!(
        env.tmux_text([
            "show-options",
            "-qv",
            "-t",
            "=native-adopted:",
            "@atelier_managed",
        ]),
        "1"
    );
}

#[test]
fn snapshot_restores_windows_and_panes() {
    let env = TestEnv::new();
    let path = env.root.path().join("restore project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "restored",
        "--detached",
    ]);
    env.ok(["window", "restored"]);
    let pane = env.tmux_text(["display-message", "-p", "-t", "=restored:1", "#{pane_id}"]);
    env.ok(["split", "horizontal", &pane]);
    env.ok(["internal", "snapshot"]);
    assert!(env.state.join("restore.snapshot").is_file());

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=restored"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    env.ok(["internal", "restore-start"]);
    assert!(env
        .tmux(["has-session", "-t", "=restored"])
        .status
        .success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=restored", "-F", "#{window_id}"])
            .lines()
            .count(),
        2
    );
    assert_eq!(
        env.tmux_text(["list-panes", "-t", "=restored:1", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );
}
