use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;
use tmux_atelier::snapshot::Snapshot;

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
        let output = self.command(&self.cli).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
        assert!(
            self.command(&self.tmux)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }

    fn tmux_text<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
    }

    fn workspace(&self, name: &str) -> PathBuf {
        self.state.join("workspaces").join(name)
    }

    fn workspace_token(&self, name: &str) -> String {
        let generation: u32 = self
            .tmux_text(["show-options", "-gqv", "@atelier_range_generation"])
            .parse()
            .unwrap();
        let prefix = format!("@atelier_range_a{generation:x}_");
        self.tmux_text(["show-options", "-g"])
            .lines()
            .find_map(|line| {
                let (option, value) = line.split_once(' ')?;
                (option.starts_with(&prefix) && value == name)
                    .then(|| option.trim_start_matches("@atelier_range_").to_owned())
            })
            .unwrap_or_else(|| panic!("missing status token for {name}"))
    }

    fn scripted(&self, responses: &str) -> PathBuf {
        let path = self.root.path().join("interaction");
        fs::write(&path, responses).unwrap();
        path
    }

    fn with_input<I, S>(&self, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = self
            .command(&self.cli)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
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
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_source_style"]),
        "default,reverse,dim"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_target_style"]),
        "underscore"
    );
    let status = env.tmux_text(["show-options", "-gqv", "@atelier_tabs_format"]);
    assert!(status.contains("::"));
    assert!(status.contains("range=window"));
    assert!(status.contains("list=focus"));
    assert!(status.contains("::#[list=on]"));
    let keys = env.tmux_text(["list-keys", "-T", "prefix"]);
    assert!(keys.contains("user-binding"));
    assert!(keys.contains("another window action"));
    assert!(keys.contains(" window \\\"#{session_name}\\\""), "{keys}");
    assert!(keys.contains("navigate-tab next"), "{keys}");
    assert!(keys.contains("navigate-tab previous"), "{keys}");
    assert!(keys.contains("navigate-workspace next"), "{keys}");
    assert!(keys.contains("navigate-workspace previous"), "{keys}");
    assert!(keys.contains("prefix N"), "{keys}");
    assert!(keys.contains("internal popup-new"), "{keys}");
    let mouse_keys = env.tmux_text(["list-keys", "-T", "root"]);
    assert!(mouse_keys.contains("MouseDown3Status"), "{mouse_keys}");
    assert!(mouse_keys.contains("internal status-menu"), "{mouse_keys}");
    assert!(mouse_keys.contains("#{window_id}"), "{mouse_keys}");
    assert!(mouse_keys.contains("MouseDown1Status"), "{mouse_keys}");
    assert!(mouse_keys.contains("internal drag-start"), "{mouse_keys}");
    assert!(mouse_keys.contains("#{client_pid}"), "{mouse_keys}");
    assert!(mouse_keys.contains("MouseDragEnd1Status"), "{mouse_keys}");
    assert!(mouse_keys.contains("internal drag-end"), "{mouse_keys}");
    assert!(mouse_keys.contains("MouseDrag1Status"), "{mouse_keys}");
    assert!(mouse_keys.contains("internal drag-update"), "{mouse_keys}");
    assert!(mouse_keys.contains("internal status-click"), "{mouse_keys}");
    let workspace_status =
        env.tmux_text(["show-options", "-qv", "-t", "=native:", "status-format[1]"]);
    assert!(workspace_status.contains("list=focus"));
    assert!(workspace_status.contains("│#[list=on]"));
    let hooks = env.tmux_text(["show-hooks", "-g"]);
    assert!(hooks.contains("internal refresh-status"));
    assert!(hooks.contains("internal snapshot"));
    assert!(hooks.contains("internal restore-start"));
    assert!(hooks.contains("internal adopt-session"));
    assert!(hooks.contains("internal cleanup-drags"));

    env.tmux_ok(["set-option", "-g", "@atelier_new_workspace_key", "off"]);
    let output = env
        .command(&env.repo.join("tmux-atelier.tmux"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let keys = env.tmux_text(["list-keys", "-T", "prefix"]);
    assert!(!keys.contains("internal popup-new"), "{keys}");
}

#[test]
fn drag_tracking_is_isolated_and_cleans_every_temporary_resource() {
    let env = TestEnv::new();
    env.tmux_ok(["new-session", "-d", "-s", "native"]);
    env.ok([
        "internal",
        "configure",
        env.repo.to_str().unwrap(),
        env.cli.to_str().unwrap(),
    ]);
    let token = env.workspace_token("native");
    let old_mapping = format!("@atelier_range_{token}");

    env.ok(["internal", "drag-start", &token, "", "301"]);
    env.ok(["internal", "drag-start", &token, "", "302"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_kind_301"]),
        "workspace"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_source_302"]),
        "native"
    );
    env.tmux_ok(["set-option", "-gq", "@atelier_drag_target_301", &token]);
    env.tmux_ok(["set-option", "-gq", "@atelier_drag_target_302", "different"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_target_301"]),
        token
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_target_302"]),
        "different"
    );

    let table = env.tmux_text(["list-keys", "-T", "atelier-drag-301"]);
    assert!(table.contains("MouseDrag1Status"), "{table}");
    assert!(table.contains("MouseDrag1Pane"), "{table}");
    assert!(table.contains("MouseDrag1ScrollbarSlider"), "{table}");
    assert!(table.contains("MouseDrag1Control9"), "{table}");
    assert!(table.contains("MouseDragEnd1StatusDefault"), "{table}");
    assert!(table.contains("@atelier_drag_target_301"), "{table}");
    assert!(table.contains("refresh-client -S"), "{table}");
    assert!(table.contains("bind-key -r"), "{table}");
    assert!(table.contains("internal drag-update"), "{table}");
    assert!(!table.contains("set-option -gqF"), "{table}");

    let generation: u32 = env
        .tmux_text(["show-options", "-gqv", "@atelier_range_generation"])
        .parse()
        .unwrap();
    env.ok(["internal", "refresh-status"]);
    env.ok(["internal", "refresh-status"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_range_generation"]),
        generation.wrapping_add(2).to_string()
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", &old_mapping]),
        "native"
    );

    env.ok(["internal", "drag-cancel", "301"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_source_301"]),
        ""
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_target_302"]),
        "different"
    );
    env.ok(["internal", "drag-cancel", "302"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_kind_302"]),
        ""
    );
    assert!(
        !env.tmux(["list-keys", "-T", "atelier-drag-302"])
            .status
            .success()
    );
}

#[test]
fn attached_client_expands_semantic_source_and_target_styles() {
    let env = TestEnv::new();
    for name in ["source", "target"] {
        let path = env.root.path().join(name);
        fs::create_dir(&path).unwrap();
        env.ok([
            "new",
            &format!("local:{}", path.display()),
            name,
            "--detached",
        ]);
    }
    let mut control = env
        .command(&env.tmux)
        .args(["-C", "attach-session", "-t", "=source"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut client = String::new();
    for _ in 0..50 {
        client = env
            .tmux_text(["list-clients", "-F", "#{client_pid}\t#{client_name}"])
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        if !client.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let (client_id, client_name) = client.split_once('\t').unwrap();
    env.ok([
        "internal",
        "configure",
        env.repo.to_str().unwrap(),
        env.cli.to_str().unwrap(),
    ]);
    let source = env.workspace_token("source");
    env.ok(["internal", "drag-start", &source, client_name, client_id]);
    let target = env.workspace_token("target");
    env.ok(["internal", "drag-update", &target, client_name, client_id]);
    let rendered = env.tmux_text([
        "display-message",
        "-p",
        "-c",
        client_name,
        "#{E:status-format[1]}",
    ]);
    assert!(rendered.contains("#[default,reverse,dim]"), "{rendered}");
    assert!(rendered.contains("#[reverse,underscore]"), "{rendered}");

    env.tmux_ok(["detach-client", "-t", client_name]);
    control.wait().unwrap();
    for _ in 0..50 {
        if env
            .tmux_text([
                "show-options",
                "-gqv",
                &format!("@atelier_drag_source_{client_id}"),
            ])
            .is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        env.tmux_text([
            "show-options",
            "-gqv",
            &format!("@atelier_drag_source_{client_id}"),
        ]),
        ""
    );
    assert!(
        !env.tmux(["list-keys", "-T", &format!("atelier-drag-{client_id}")])
            .status
            .success()
    );
}

#[test]
fn tab_navigation_selects_adjacent_windows() {
    let env = TestEnv::new();
    let path = env.root.path().join("tabs");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "tabs",
        "--detached",
    ]);
    env.ok(["window", "tabs"]);

    assert_eq!(
        env.tmux_text(["display-message", "-p", "-t", "=tabs:", "#{window_index}"]),
        "1"
    );
    env.ok(["internal", "navigate-tab", "previous", "tabs"]);
    assert_eq!(
        env.tmux_text(["display-message", "-p", "-t", "=tabs:", "#{window_index}"]),
        "0"
    );
    env.ok(["internal", "navigate-tab", "next", "tabs"]);
    assert_eq!(
        env.tmux_text(["display-message", "-p", "-t", "=tabs:", "#{window_index}"]),
        "1"
    );
}

#[test]
fn workspace_drag_order_survives_rename_delete_and_native_sessions() {
    let env = TestEnv::new();
    for name in ["one", "two", "three"] {
        let path = env.root.path().join(name);
        fs::create_dir(&path).unwrap();
        env.ok([
            "new",
            &format!("local:{}", path.display()),
            name,
            "--detached",
        ]);
    }
    env.ok([
        "internal",
        "configure",
        env.repo.to_str().unwrap(),
        env.cli.to_str().unwrap(),
    ]);
    let source = env.workspace_token("one");
    let target = env.workspace_token("three");
    env.ok(["internal", "drag-start", &source, "", "101"]);
    env.ok(["internal", "drag-end", &target, "", "101"]);
    let order = env.state.join("workspace.order");
    assert_eq!(fs::read_to_string(&order).unwrap(), "two\nthree\none\n");
    assert_eq!(
        fs::metadata(&order).unwrap().permissions().mode() & 0o777,
        0o600
    );

    env.ok(["rename", "three", "renamed"]);
    assert_eq!(fs::read_to_string(&order).unwrap(), "two\nrenamed\none\n");
    env.ok(["delete", "renamed"]);
    assert_eq!(fs::read_to_string(&order).unwrap(), "two\none\n");

    env.tmux_ok(["set-hook", "-gu", "session-created[91]"]);
    env.tmux_ok(["new-session", "-d", "-s", "native"]);
    env.ok(["internal", "refresh-status"]);
    let source = env.workspace_token("native");
    let target = env.workspace_token("two");
    env.ok(["internal", "drag-start", &source, "", "102"]);
    env.ok(["internal", "drag-end", &target, "", "102"]);
    assert_eq!(fs::read_to_string(order).unwrap(), "native\ntwo\none\n");
}

#[test]
fn tab_drag_inserts_real_windows_snapshots_and_keeps_plain_clicks() {
    let env = TestEnv::new();
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    let path = env.root.path().join("tab-drag");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "tab-drag",
        "--detached",
    ]);
    env.ok(["window", "tab-drag"]);
    env.ok(["window", "tab-drag"]);
    env.ok([
        "internal",
        "configure",
        env.repo.to_str().unwrap(),
        env.cli.to_str().unwrap(),
    ]);
    let windows = env
        .tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_id}"])
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for (window, name) in windows.iter().zip(["first", "second", "third"]) {
        env.tmux_ok(["rename-window", "-t", window, name]);
    }
    let original_indexes =
        env.tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_index}"]);

    env.ok(["internal", "drag-start", "window", "", "201", &windows[0]]);
    env.ok(["internal", "drag-end", "window", "", "201", &windows[2]]);
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_id}",])
            .lines()
            .collect::<Vec<_>>(),
        [&windows[1], &windows[2], &windows[0]]
    );
    env.ok(["internal", "drag-start", "window", "", "202", &windows[2]]);
    env.ok(["internal", "drag-end", "window", "", "202", &windows[1]]);
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_id}",])
            .lines()
            .collect::<Vec<_>>(),
        [&windows[2], &windows[1], &windows[0]]
    );
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_index}",]),
        original_indexes
    );
    let saved = Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    let live_indexes = env
        .tmux_text(["list-windows", "-t", "=tab-drag", "-F", "#{window_index}"])
        .lines()
        .map(|index| index.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        saved.workspaces[0]
            .windows
            .iter()
            .map(|window| window.index)
            .collect::<Vec<_>>(),
        live_indexes
    );
    assert_eq!(
        saved.workspaces[0]
            .windows
            .iter()
            .map(|window| window.name.as_str())
            .collect::<Vec<_>>(),
        ["third", "second", "first"]
    );

    env.ok(["internal", "drag-start", "window", "", "203", &windows[2]]);
    env.ok(["internal", "drag-update", "window", "", "203"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_target_203"]),
        ""
    );
    env.ok([
        "internal",
        "status-click",
        "window",
        "",
        "203",
        "tab-drag",
        &windows[2],
    ]);
    assert_eq!(
        env.tmux_text(["display-message", "-p", "-t", "=tab-drag:", "#{window_id}",]),
        windows[2]
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_drag_source_203"]),
        ""
    );

    thread::sleep(Duration::from_millis(200));
    let drag_snapshot = fs::read(env.state.join("restore.snapshot")).unwrap();
    env.tmux_ok(["kill-session", "-t", "=tab-drag"]);
    thread::sleep(Duration::from_millis(200));
    fs::write(env.state.join("restore.snapshot"), drag_snapshot).unwrap();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.tmux_ok(["set-option", "-gu", "@atelier_restore_handled"]);
    env.tmux_ok(["set-option", "-gu", "@atelier_restore_pending"]);
    env.tmux_ok(["set-option", "-gu", "@atelier_restore_started"]);
    env.ok(["internal", "restore-arm"]);
    env.ok(["internal", "restore-start"]);
    assert_eq!(
        env.tmux_text([
            "list-windows",
            "-t",
            "=tab-drag",
            "-F",
            "#{window_index}\t#{window_name}",
        ]),
        live_indexes
            .iter()
            .zip(["third", "second", "first"])
            .map(|(index, name)| format!("{index}\t{name}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn tab_drag_preserves_sparse_window_indexes() {
    let env = TestEnv::new();
    let path = env.root.path().join("sparse-tab-drag");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "sparse",
        "--detached",
    ]);
    env.ok(["window", "sparse"]);
    env.ok(["window", "sparse"]);
    env.tmux_ok(["move-window", "-s", "=sparse:2", "-t", "=sparse:5"]);
    env.tmux_ok(["move-window", "-s", "=sparse:1", "-t", "=sparse:3"]);
    env.tmux_ok(["move-window", "-s", "=sparse:0", "-t", "=sparse:1"]);
    env.ok(["internal", "snapshot"]);
    env.ok([
        "internal",
        "configure",
        env.repo.to_str().unwrap(),
        env.cli.to_str().unwrap(),
    ]);
    let windows = env
        .tmux_text(["list-windows", "-t", "=sparse", "-F", "#{window_id}"])
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    env.ok(["internal", "drag-start", "window", "", "204", &windows[0]]);
    env.ok(["internal", "drag-end", "window", "", "204", &windows[2]]);
    assert_eq!(
        env.tmux_text([
            "list-windows",
            "-t",
            "=sparse",
            "-F",
            "#{window_index}\t#{window_id}",
        ]),
        format!("1\t{}\n3\t{}\n5\t{}", windows[1], windows[2], windows[0])
    );
}

#[test]
fn tab_menu_actions_rename_and_delete_windows() {
    let env = TestEnv::new();
    let path = env.root.path().join("tab-actions");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "tab-actions",
        "--detached",
    ]);
    env.ok(["window", "tab-actions"]);
    let window = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=tab-actions:1",
        "#{window_id}",
    ]);

    let rename = env.scripted("input\trenamed tab\n");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", rename)
        .args(["internal", "popup-tab-rename", &window])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        env.tmux_text(["display-message", "-p", "-t", &window, "#{window_name}"]),
        "renamed tab"
    );

    let reject = env.scripted("confirm\tfalse\n");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", reject)
        .args(["internal", "confirm-tab-close", &window])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        env.tmux(["display-message", "-p", "-t", &window])
            .status
            .success()
    );

    let accept = env.scripted("confirm\ttrue\n");
    let output = env
        .command(&env.cli)
        .env("TMUX_ATELIER_INTERACTION_FILE", accept)
        .args(["internal", "confirm-tab-close", &window])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !env.tmux_text(["list-windows", "-t", "=tab-actions", "-F", "#{window_id}"])
            .lines()
            .any(|id| id == window)
    );
}

#[test]
fn terminal_confirmation_is_written_to_stderr() {
    let env = TestEnv::new();

    let output = env.with_input(["internal", "confirm-close", "workspace"], "n\n");

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Stop workspace workspace? [Y/n] "
    );
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
    assert!(
        fs::read_to_string(env.workspace("collision"))
            .unwrap()
            .contains(&format!("path={}", collision.display()))
    );
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
    assert!(
        fs::read_to_string(env.workspace("from-history"))
            .unwrap()
            .contains("destination=history@winhost")
    );
    assert!(
        fs::read_to_string(history_log)
            .unwrap()
            .contains("Modèles/")
    );

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
    env.tmux_ok(["move-window", "-s", "=restored:1", "-t", "=restored:3"]);
    env.ok(["internal", "snapshot"]);
    assert!(env.state.join("restore.snapshot").is_file());

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=restored"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    let restore = env.cli(["internal", "restore-start"]);
    assert!(
        restore.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&restore.stderr)
    );
    assert!(
        env.tmux(["has-session", "-t", "=restored"])
            .status
            .success()
    );
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=restored", "-F", "#{window_id}"])
            .lines()
            .count(),
        2
    );
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=restored", "-F", "#{window_index}"]),
        "0\n3"
    );
    assert_eq!(
        env.tmux_text(["list-panes", "-t", "=restored:3", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );
}

#[test]
fn restore_prompt_is_skipped_when_saved_sessions_already_exist() {
    let env = TestEnv::new();
    let path = env.root.path().join("existing restore project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "existing",
        "--detached",
    ]);
    env.ok(["internal", "snapshot"]);

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_handled", "0"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_pending", "1"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_started", "0"]);
    env.ok(["internal", "restore-start"]);

    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_handled"]),
        "1",
        "{}",
        fs::read_to_string(env.state.join("debug.log")).unwrap_or_default()
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "0"
    );
}

#[test]
fn interrupted_replacements_restore_the_original_session() {
    let env = TestEnv::new();
    let path = env.root.path().join("recovery project");
    fs::create_dir(&path).unwrap();
    env.tmux_ok([
        "new-session",
        "-d",
        "-s",
        "recover",
        "-c",
        path.to_str().unwrap(),
    ]);
    let original_pane =
        env.tmux_text(["display-message", "-p", "-t", "=recover:0.0", "#{pane_id}"]);

    for phase in 0..4 {
        env.tmux_ok([
            "set-option",
            "-gq",
            "@atelier_restore_transaction_phase",
            "test|prepared",
        ]);
        env.tmux_ok([
            "set-option",
            "-q",
            "-t",
            "=recover:",
            "@atelier_restore_transaction",
            "2|test|recover|restore-stage|restore-backup",
        ]);
        if phase > 0 {
            env.tmux_ok(["new-session", "-d", "-s", "restore-stage"]);
            env.tmux_ok([
                "set-option",
                "-q",
                "-t",
                "=restore-stage:",
                "@atelier_restore_owner",
                "test",
            ]);
        }
        if phase > 1 {
            env.tmux_ok(["rename-session", "-t", "=recover", "restore-backup"]);
        }
        if phase > 2 {
            env.tmux_ok(["rename-session", "-t", "=restore-stage", "recover"]);
        }
        env.ok(["internal", "restore-arm"]);
        assert_eq!(
            env.tmux_text(["display-message", "-p", "-t", "=recover:0.0", "#{pane_id}",]),
            original_pane
        );
        assert!(
            !env.tmux(["has-session", "-t", "=restore-stage"])
                .status
                .success()
        );
        assert!(
            !env.tmux(["has-session", "-t", "=restore-backup"])
                .status
                .success()
        );
        assert_eq!(
            env.tmux_text([
                "show-options",
                "-qv",
                "-t",
                "=recover:",
                "@atelier_restore_transaction",
            ]),
            ""
        );
    }

    env.tmux_ok(["new-session", "-d", "-s", "native-stage"]);
    env.tmux_ok([
        "set-option",
        "-gq",
        "@atelier_restore_transaction_phase",
        "test|prepared",
    ]);
    env.tmux_ok([
        "set-option",
        "-q",
        "-t",
        "=recover:",
        "@atelier_restore_transaction",
        "2|test|recover|native-stage|restore-backup",
    ]);
    env.ok(["internal", "restore-arm"]);
    assert!(
        env.tmux(["has-session", "-t", "=native-stage"])
            .status
            .success()
    );
}

#[test]
fn committed_replacement_cleanup_never_rolls_back_remaining_backups() {
    let env = TestEnv::new();
    for suffix in ["one", "two"] {
        env.tmux_ok(["new-session", "-d", "-s", &format!("original-{suffix}")]);
        env.tmux_ok(["new-session", "-d", "-s", &format!("staged-{suffix}")]);
        env.tmux_ok([
            "set-option",
            "-q",
            "-t",
            &format!("=original-{suffix}:"),
            "@atelier_restore_transaction",
            &format!("2|test|original-{suffix}|staged-{suffix}|backup-{suffix}"),
        ]);
        env.tmux_ok([
            "set-option",
            "-q",
            "-t",
            &format!("=staged-{suffix}:"),
            "@atelier_restore_owner",
            "test",
        ]);
        env.tmux_ok([
            "rename-session",
            "-t",
            &format!("=original-{suffix}"),
            &format!("backup-{suffix}"),
        ]);
        env.tmux_ok([
            "rename-session",
            "-t",
            &format!("=staged-{suffix}"),
            &format!("original-{suffix}"),
        ]);
    }
    env.tmux_ok([
        "set-option",
        "-gq",
        "@atelier_restore_transaction_phase",
        "test|committed",
    ]);
    env.tmux_ok(["kill-session", "-t", "=backup-one"]);

    env.ok(["internal", "restore-arm"]);

    for suffix in ["one", "two"] {
        assert!(
            env.tmux(["has-session", "-t", &format!("=original-{suffix}")])
                .status
                .success()
        );
        assert!(
            !env.tmux(["has-session", "-t", &format!("=backup-{suffix}")])
                .status
                .success()
        );
    }
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "0"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_handled"]),
        "1"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_transaction_phase",]),
        ""
    );
}

#[test]
fn deleted_pane_directory_restores_at_workspace_root() {
    let env = TestEnv::new();
    let root = env.root.path().join("path fallback project");
    let nested = root.join("removed");
    fs::create_dir_all(&nested).unwrap();
    env.ok([
        "new",
        &format!("local:{}", root.display()),
        "path-fallback",
        "--detached",
    ]);
    env.tmux_ok([
        "send-keys",
        "-l",
        "-t",
        "=path-fallback:0.0",
        &format!("cd {}", nested.display()),
    ]);
    env.tmux_ok(["send-keys", "-t", "=path-fallback:0.0", "Enter"]);
    thread::sleep(Duration::from_millis(100));
    env.ok(["internal", "snapshot"]);
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=path-fallback"]);
    fs::remove_dir(&nested).unwrap();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    env.ok(["internal", "restore-start"]);

    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=path-fallback:0.0",
            "#{pane_current_path}",
        ]),
        root.to_string_lossy()
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "0"
    );
}

#[test]
fn deleted_saved_directory_matches_an_existing_root_pane() {
    let env = TestEnv::new();
    let root = env.root.path().join("existing path fallback project");
    let nested = root.join("removed");
    fs::create_dir_all(&nested).unwrap();
    env.ok([
        "new",
        &format!("local:{}", root.display()),
        "existing-path-fallback",
        "--detached",
    ]);
    env.tmux_ok([
        "send-keys",
        "-l",
        "-t",
        "=existing-path-fallback:0.0",
        &format!("cd {}", nested.display()),
    ]);
    env.tmux_ok(["send-keys", "-t", "=existing-path-fallback:0.0", "Enter"]);
    thread::sleep(Duration::from_millis(100));
    env.ok(["internal", "snapshot"]);
    let pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=existing-path-fallback:0.0",
        "#{pane_id}",
    ]);
    env.tmux_ok([
        "send-keys",
        "-l",
        "-t",
        &pane,
        &format!("cd {}", root.display()),
    ]);
    env.tmux_ok(["send-keys", "-t", &pane, "Enter"]);
    thread::sleep(Duration::from_millis(100));
    fs::remove_dir(&nested).unwrap();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_handled", "0"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_pending", "1"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_started", "0"]);
    env.ok(["internal", "restore-start"]);

    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=existing-path-fallback:0.0",
            "#{pane_id}",
        ]),
        pane
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_handled"]),
        "1"
    );
}

#[test]
fn internal_process_launcher_preserves_custom_and_empty_argv0() {
    let env = TestEnv::new();
    for (name, argv0) in [("custom", "custom-name"), ("empty", "")] {
        let output = env.root.path().join(name);
        let script = format!("printf %s \"$0\" > {}", output.display());
        let result = env.cli([
            "internal",
            "process-exec",
            "--executable",
            "/bin/sh",
            "--",
            argv0,
            "-c",
            &script,
        ]);
        assert!(result.status.success());
        assert_eq!(fs::read_to_string(output).unwrap(), argv0);
    }
}

#[test]
fn unsupported_shell_skips_recipe_without_failing_restore() {
    let env = TestEnv::new();
    fs::write(env.root.path().join(".zshrc"), "").unwrap();
    let path = env.root.path().join("unsupported shell project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "unsupported-shell",
        "--detached",
    ]);
    let pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=unsupported-shell:0.0",
        "#{pane_id}",
    ]);
    env.ok(["restart-policy", "always", &pane]);
    env.tmux_ok(["send-keys", "-l", "-t", &pane, "sleep 30"]);
    env.tmux_ok(["send-keys", "-t", &pane, "Enter"]);
    thread::sleep(Duration::from_millis(200));
    env.ok(["internal", "snapshot"]);
    let snapshot_path = env.state.join("restore.snapshot");
    let mut saved = Snapshot::decode(&fs::read(&snapshot_path).unwrap()).unwrap();
    saved.workspaces[0].windows[0].panes[0]
        .shell
        .as_mut()
        .unwrap()
        .executable = "/bin/sh".into();
    fs::write(&snapshot_path, saved.encode()).unwrap();
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=unsupported-shell"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    let restored = env.cli(["internal", "restore-start"]);
    assert!(restored.status.success());
    assert!(String::from_utf8_lossy(&restored.stderr).contains("unsupported"));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=unsupported-shell:0.0",
            "#{pane_current_command}",
        ]),
        "sh"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "0"
    );
}

#[test]
fn legacy_two_option_shell_state_is_still_captured() {
    let env = TestEnv::new();
    let path = env.root.path().join("legacy shell project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "legacy-shell",
        "--detached",
    ]);
    let pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=legacy-shell:0.0",
        "#{pane_id}",
    ]);
    env.tmux_ok([
        "set-option",
        "-pq",
        "-t",
        &pane,
        "@atelier_restart_shell",
        "/bin/bash",
    ]);
    env.tmux_ok([
        "set-option",
        "-pq",
        "-t",
        &pane,
        "@atelier_restart_shell_login",
        "1",
    ]);
    env.ok(["internal", "snapshot"]);
    let saved = Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    assert_eq!(
        saved.workspaces[0].windows[0].panes[0]
            .shell
            .as_ref()
            .unwrap(),
        &tmux_atelier::process_state::SavedShell {
            executable: "/bin/bash".into(),
            login: true,
        }
    );
}

#[test]
fn atelier_actions_are_blocked_while_restoration_is_pending() {
    let env = TestEnv::new();
    let path = env.root.path().join("pending project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "pending",
        "--detached",
    ]);
    env.ok(["internal", "snapshot"]);
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=pending"]);
    env.ok(["internal", "restore-arm"]);

    let output = env.cli(["open", "pending", "--detached"]);
    assert!(!output.status.success());
    assert!(!env.tmux(["has-session", "-t", "=pending"]).status.success());
}

#[test]
fn confirmed_restoration_replaces_a_mismatched_workspace() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);

    let output = env.with_input(["internal", "popup-restore"], "y\ny\n");
    assert!(output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        2
    );
    assert_eq!(
        env.tmux_text(["list-panes", "-t", "=mismatch:1", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );
    assert!(
        !env.tmux_text(["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|name| name.starts_with("atelier-restore-"))
    );
    assert_eq!(
        env.tmux_text([
            "show-options",
            "-qv",
            "-t",
            "=mismatch:",
            "@atelier_restore_transaction",
        ]),
        ""
    );
    assert_eq!(
        env.tmux_text([
            "show-options",
            "-qv",
            "-t",
            "=mismatch:",
            "@atelier_restore_owner",
        ]),
        ""
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_transaction_phase",]),
        ""
    );
}

#[test]
fn confirmed_restoration_commits_multiple_replacements_together() {
    let env = TestEnv::new();
    for name in ["replace-one", "replace-two"] {
        let path = env.root.path().join(name);
        fs::create_dir(&path).unwrap();
        env.ok([
            "new",
            &format!("local:{}", path.display()),
            name,
            "--detached",
        ]);
        env.tmux_ok(["new-window", "-d", "-t", &format!("={name}:")]);
    }
    env.ok(["internal", "snapshot"]);
    let saved = fs::read(env.state.join("restore.snapshot")).unwrap();
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    for name in ["replace-one", "replace-two"] {
        env.tmux_ok(["kill-session", "-t", &format!("={name}")]);
        env.ok(["open", name, "--detached"]);
    }
    fs::write(env.state.join("restore.snapshot"), saved).unwrap();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    env.ok(["internal", "restore-arm"]);

    let restored = env.with_input(["internal", "popup-restore"], "y\ny\n");
    assert!(restored.status.success());
    for name in ["replace-one", "replace-two"] {
        assert_eq!(
            env.tmux_text([
                "list-windows",
                "-t",
                &format!("={name}"),
                "-F",
                "#{window_id}"
            ])
            .lines()
            .count(),
            2
        );
    }
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_transaction_phase",]),
        ""
    );
}

#[test]
fn declining_replacement_starts_fresh_without_changing_the_live_workspace() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);

    let output = env.with_input(["internal", "popup-restore"], "y\nn\n");
    assert!(output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "0"
    );
}

#[test]
fn canceling_restoration_keeps_the_snapshot_pending() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    let snapshot = fs::read(env.state.join("restore.snapshot")).unwrap();

    let output = env.with_input(["internal", "popup-restore"], "");

    assert!(output.status.success());
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "1"
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_handled"]),
        "0"
    );
    assert_eq!(
        fs::read(env.state.join("restore.snapshot")).unwrap(),
        snapshot
    );
}

#[test]
fn always_mode_waits_for_confirmation_before_replacing_a_live_workspace() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);

    env.ok(["internal", "restore-start"]);
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "1"
    );
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );

    let output = env.with_input(["internal", "popup-restore"], "y\n");
    assert!(output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        2
    );
}

#[test]
fn direct_internal_restore_cannot_replace_an_unconfirmed_workspace() {
    let env = mismatched_restore_env();

    let output = env.cli(["internal", "restore"]);
    assert!(!output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        env.tmux_text(["show-options", "-gqv", "@atelier_restore_pending"]),
        "1"
    );
}

#[test]
fn confirmed_workspace_is_rejected_if_its_topology_changes() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    let mut child = env
        .command(&env.cli)
        .args(["internal", "popup-restore"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stderr.take().unwrap();
    let mut byte = [0];
    while output.read_exact(&mut byte).is_ok() && byte[0] != b'?' {}
    input.write_all(b"y\n").unwrap();
    input.flush().unwrap();
    while output.read_exact(&mut byte).is_ok() && byte[0] != b'?' {}

    env.tmux_ok(["new-window", "-d", "-t", "=mismatch:"]);
    let window_count_before = env
        .tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
        .lines()
        .count();
    input.write_all(b"y\n").unwrap();
    drop(input);
    let status = child.wait().unwrap();

    assert!(!status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        window_count_before
    );
}

#[test]
fn process_change_during_topology_staging_skips_every_process_launch() {
    let env = TestEnv::new();
    fs::write(env.root.path().join(".zshrc"), "").unwrap();
    let slow_path = env.root.path().join("slow staging project");
    let process_path = env.root.path().join("stale process project");
    fs::create_dir(&slow_path).unwrap();
    fs::create_dir(&process_path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", slow_path.display()),
        "slow-stage",
        "--detached",
    ]);
    for _ in 0..15 {
        env.tmux_ok(["new-window", "-d", "-t", "=slow-stage:"]);
    }
    env.ok([
        "new",
        &format!("local:{}", process_path.display()),
        "stale-process",
        "--detached",
    ]);
    let process_pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=stale-process:0.0",
        "#{pane_id}",
    ]);
    env.tmux_ok(["send-keys", "-l", "-t", &process_pane, "sleep 30"]);
    env.tmux_ok(["send-keys", "-t", &process_pane, "Enter"]);
    thread::sleep(Duration::from_millis(200));
    env.ok(["restart-policy", "always", &process_pane]);
    let saved = fs::read(env.state.join("restore.snapshot")).unwrap();

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=slow-stage"]);
    env.ok(["open", "slow-stage", "--detached"]);
    fs::write(env.state.join("restore.snapshot"), saved).unwrap();
    env.tmux_ok(["send-keys", "-t", &process_pane, "C-c"]);
    thread::sleep(Duration::from_millis(100));
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    env.ok(["internal", "restore-arm"]);

    let mut child = env
        .command(&env.cli)
        .args(["internal", "popup-restore"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\ny\n").unwrap();
    let mut staging_started = false;
    for _ in 0..200 {
        if env
            .tmux_text(["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|name| name.starts_with("atelier-restore-"))
        {
            staging_started = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(staging_started);
    env.tmux_ok(["send-keys", "-l", "-t", &process_pane, "sleep 25"]);
    env.tmux_ok(["send-keys", "-t", &process_pane, "Enter"]);
    let restored = child.wait_with_output().unwrap();
    assert!(restored.status.success());
    assert!(String::from_utf8_lossy(&restored.stderr).contains("skipped process restoration"));
    thread::sleep(Duration::from_millis(100));
    env.ok(["internal", "snapshot"]);
    let current = Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    let pane = &current
        .workspaces
        .iter()
        .find(|workspace| workspace.name == "stale-process")
        .unwrap()
        .windows[0]
        .panes[0];
    assert_eq!(pane.process.as_ref().unwrap().argv, ["sleep", "25"]);
}

#[test]
fn topology_change_during_later_staging_preserves_every_original() {
    let env = TestEnv::new();
    let first_path = env.root.path().join("first staged project");
    let second_path = env.root.path().join("second staged project");
    fs::create_dir(&first_path).unwrap();
    fs::create_dir(&second_path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", first_path.display()),
        "first-stage",
        "--detached",
    ]);
    env.tmux_ok(["new-window", "-d", "-t", "=first-stage:"]);
    env.ok([
        "new",
        &format!("local:{}", second_path.display()),
        "second-stage",
        "--detached",
    ]);
    for _ in 0..15 {
        env.tmux_ok(["new-window", "-d", "-t", "=second-stage:"]);
    }
    env.ok(["internal", "snapshot"]);
    let saved = fs::read(env.state.join("restore.snapshot")).unwrap();
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    for name in ["first-stage", "second-stage"] {
        env.tmux_ok(["kill-session", "-t", &format!("={name}")]);
        env.ok(["open", name, "--detached"]);
    }
    fs::write(env.state.join("restore.snapshot"), saved).unwrap();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    env.ok(["internal", "restore-arm"]);

    let mut child = env
        .command(&env.cli)
        .args(["internal", "popup-restore"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\ny\n").unwrap();
    let mut staging_started = false;
    for _ in 0..300 {
        let staging = env
            .tmux_text(["list-sessions", "-F", "#{session_name}"])
            .lines()
            .filter(|name| name.starts_with("atelier-restore-"))
            .count();
        if staging >= 1 {
            staging_started = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(staging_started);
    env.tmux_ok(["new-window", "-d", "-t", "=first-stage:"]);
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=first-stage", "-F", "#{window_id}"])
            .lines()
            .count(),
        2
    );
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=second-stage", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    assert!(
        !env.tmux_text(["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|name| name.starts_with("atelier-restore-"))
    );
}

#[test]
fn failed_staging_preserves_the_mismatched_live_workspace() {
    let env = mismatched_restore_env();
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "prompt"]);
    let snapshot_path = env.state.join("restore.snapshot");
    let mut saved = Snapshot::decode(&fs::read(&snapshot_path).unwrap()).unwrap();
    saved.workspaces[0].windows[0].layout = "invalid-layout".into();
    fs::write(snapshot_path, saved.encode()).unwrap();

    let output = env.with_input(["internal", "popup-restore"], "y\ny\n");
    assert!(!output.status.success());
    assert_eq!(
        env.tmux_text(["list-windows", "-t", "=mismatch", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    assert!(
        !env.tmux_text(["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|name| name.starts_with("atelier-restore-"))
    );
}

#[test]
fn snapshot_restores_foreground_process_and_returns_to_shell() {
    let env = TestEnv::new();
    fs::write(env.root.path().join(".zshrc"), "").unwrap();
    let path = env.root.path().join("process restore project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "process-restore",
        "--detached",
    ]);
    let pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=process-restore:0",
        "#{pane_id}",
    ]);
    let shell = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_current_command}",
    ]);
    thread::sleep(Duration::from_millis(200));
    env.tmux_ok(["send-keys", "-l", "-t", &pane, "sleep 30"]);
    env.tmux_ok(["send-keys", "-t", &pane, "Enter"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restart_min_runtime", "1"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restart_denylist", "sleep"]);
    thread::sleep(Duration::from_millis(1200));
    let current = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_current_command}",
    ]);
    let output = env.tmux_text(["capture-pane", "-p", "-t", &pane]);
    assert_eq!(current, "sleep", "pane output:\n{output}");
    env.ok(["internal", "snapshot"]);
    let automatic =
        Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    assert!(
        automatic.workspaces[0].windows[0].panes[0]
            .process
            .is_none()
    );
    let capture_log = fs::read_to_string(env.state.join("debug.log")).unwrap();
    assert!(capture_log.contains("process capture pane="));
    assert!(capture_log.contains(
        "policy=auto decision=denylisted program=sleep executable=/usr/bin/sleep arguments=2 denied=/usr/bin/sleep"
    ));

    env.ok(["restart-policy", "always", &pane]);

    let saved = Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    let state = &saved.workspaces[0].windows[0].panes[0];
    assert!(state.process.is_some(), "captured pane state: {state:?}");
    assert_eq!(state.process.as_ref().unwrap().argv, ["sleep", "30"]);

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=process-restore"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    let restore = env.cli(["internal", "restore-start"]);
    assert!(
        restore.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&restore.stderr)
    );
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=process-restore:0.0",
            "#{pane_current_command}",
        ]),
        "sleep"
    );
    assert!(
        env.tmux_text([
            "show-options",
            "-pqv",
            "-t",
            "=process-restore:0.0",
            "@atelier_restart_shell_state",
        ])
        .starts_with("1|")
    );
    assert_eq!(
        env.tmux_text([
            "show-options",
            "-pqv",
            "-t",
            "=process-restore:0.0",
            "@atelier_restart_shell",
        ]),
        ""
    );
    env.ok(["internal", "snapshot"]);
    let restored =
        Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    assert_eq!(
        restored.workspaces[0].windows[0].panes[0]
            .shell
            .as_ref()
            .unwrap(),
        state.shell.as_ref().unwrap()
    );

    env.tmux_ok(["send-keys", "-t", "=process-restore:0.0", "C-c"]);
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        env.tmux_text([
            "display-message",
            "-p",
            "-t",
            "=process-restore:0.0",
            "#{pane_current_command}",
        ]),
        shell
    );
    let restore_log = fs::read_to_string(env.state.join("debug.log")).unwrap();
    for event in [
        "restore pane planned location=process-restore:0.0 action=launch-process kind=process program=sleep executable=/usr/bin/sleep arguments=2",
        "restore pane launch requested location=process-restore:0.0",
        "restore pane launch accepted location=process-restore:0.0",
        "process launcher starting program=sleep executable=/usr/bin/sleep arguments=2",
        "process launcher spawned program=sleep child_pid=",
        "process launcher terminal handed off program=sleep child_pid=",
        "process launcher exited program=sleep child_pid=",
        "signal=2",
        "process launcher returning to shell program=sleep shell=",
    ] {
        assert!(
            restore_log.contains(event),
            "missing log event: {event}\n{restore_log}"
        );
    }

    env.tmux_ok([
        "send-keys",
        "-l",
        "-t",
        "=process-restore:0.0",
        "sleep 30 | cat",
    ]);
    env.tmux_ok(["send-keys", "-t", "=process-restore:0.0", "Enter"]);
    thread::sleep(Duration::from_millis(200));
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_pending", "1"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore_handled", "0"]);
    let unconfirmed = env.cli(["internal", "restore"]);
    assert!(!unconfirmed.status.success());
    assert!(
        env.tmux(["has-session", "-t", "=process-restore"])
            .status
            .success()
    );
}

#[test]
fn captured_shebang_script_restarts_through_its_interpreter() {
    let env = TestEnv::new();
    fs::write(env.root.path().join(".zshrc"), "").unwrap();
    let path = env.root.path().join("script restore project");
    fs::create_dir(&path).unwrap();
    let script = env.root.path().join("restart-script");
    let marker = env.root.path().join("script-marker");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf %s \"$1\" > {}\nsleep 30\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "script-restore",
        "--detached",
    ]);
    let pane = env.tmux_text([
        "display-message",
        "-p",
        "-t",
        "=script-restore:0.0",
        "#{pane_id}",
    ]);
    env.tmux_ok([
        "send-keys",
        "-l",
        "-t",
        &pane,
        &format!("{} secret-process-argument", script.display()),
    ]);
    env.tmux_ok(["send-keys", "-t", &pane, "Enter"]);
    let wait_for_marker = || {
        for _ in 0..2000 {
            if matches!(
                fs::read_to_string(&marker),
                Ok(contents) if contents == "secret-process-argument"
            ) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    };
    assert!(wait_for_marker(), "initial script did not write its marker");
    env.ok(["restart-policy", "always", &pane]);
    let saved = Snapshot::decode(&fs::read(env.state.join("restore.snapshot")).unwrap()).unwrap();
    let argument_count = saved.workspaces[0].windows[0].panes[0]
        .process
        .as_ref()
        .unwrap()
        .argv
        .len();
    let log = fs::read_to_string(env.state.join("debug.log")).unwrap();
    assert!(log.contains("policy=always decision=restartable"));
    assert!(log.contains(&format!("arguments={argument_count}")));
    assert!(!log.contains("secret-process-argument"));
    fs::remove_file(&marker).unwrap();
    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=script-restore"]);
    env.tmux_ok(["set-option", "-gq", "@atelier_restore", "always"]);
    env.ok(["internal", "restore-arm"]);
    let restore = env.cli(["internal", "restore-start"]);
    assert!(
        restore.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&restore.stderr)
    );
    assert!(
        wait_for_marker(),
        "restored script did not write its marker: {}\n{}",
        String::from_utf8_lossy(&restore.stderr),
        fs::read_to_string(env.state.join("debug.log")).unwrap_or_default()
    );
}

fn mismatched_restore_env() -> TestEnv {
    let env = TestEnv::new();
    let path = env.root.path().join("mismatched restore project");
    fs::create_dir(&path).unwrap();
    env.ok([
        "new",
        &format!("local:{}", path.display()),
        "mismatch",
        "--detached",
    ]);
    env.ok(["window", "mismatch"]);
    let pane = env.tmux_text(["display-message", "-p", "-t", "=mismatch:1", "#{pane_id}"]);
    env.ok(["split", "horizontal", &pane]);
    env.ok(["internal", "snapshot"]);
    let saved = fs::read(env.state.join("restore.snapshot")).unwrap();

    env.tmux_ok(["new-session", "-d", "-s", "bootstrap"]);
    env.tmux_ok(["kill-session", "-t", "=mismatch"]);
    env.ok(["open", "mismatch", "--detached"]);
    fs::write(env.state.join("restore.snapshot"), saved).unwrap();
    env.ok(["internal", "restore-arm"]);
    env
}
