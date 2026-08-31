use std::ffi::OsStr;
use std::process::{Command, Output, Stdio};

use crate::config::{Config, quote_powershell, quote_sh};
use crate::{Result, err};

pub fn run<I, S>(program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(format!(
            "command failed with status {status}: {program}"
        )))
    }
}

pub fn output<I, S>(program: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    checked_output(Command::new(program).args(args).output()?, program)
}

fn checked_output(result: Output, program: &str) -> Result<String> {
    if !result.status.success() {
        return Err(err(format!(
            "command failed with status {}: {program}",
            result.status
        )));
    }
    Ok(String::from_utf8(result.stdout)?
        .trim_end_matches('\n')
        .to_owned())
}

pub fn output_quiet<I, S>(program: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| out.status.success().then_some(out.stdout))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim_end_matches('\n').to_owned())
}

pub fn tmux(config: &Config, args: &[&str]) -> Result<()> {
    run(&config.tmux, args)
}

pub fn tmux_output(config: &Config, args: &[&str]) -> Result<String> {
    output(&config.tmux, args)
}

pub fn tmux_quiet(config: &Config, args: &[&str]) -> Option<String> {
    output_quiet(&config.tmux, args)
}

pub fn tmux_success(config: &Config, args: &[&str]) -> bool {
    Command::new(&config.tmux)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub fn ssh_args(config: &Config) -> Result<Vec<String>> {
    let directory = config.state_root.join("ssh");
    config.secure_dir(&directory)?;
    Ok(vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        "ControlPersist=60".into(),
        "-o".into(),
        format!("ControlPath={}/%C", directory.display()),
    ])
}

pub fn ssh_output(config: &Config, destination: &str, remote: &str, quiet: bool) -> Result<String> {
    let mut args = ssh_args(config)?;
    args.push(destination.into());
    args.push(remote.into());
    let mut command = Command::new("ssh");
    command.args(&args);
    if quiet {
        command.stderr(Stdio::null());
    }
    checked_output(command.output()?, "ssh")
}

pub fn remote_shell_command(
    config: &Config,
    destination: &str,
    path: &str,
    shell: &str,
) -> Result<String> {
    let remote = match shell {
        "powershell" => format!(
            "powershell.exe -NoLogo -NoExit -Command \"Set-Location -LiteralPath {}\"",
            quote_powershell(path)
        ),
        "windows" => format!(
            "powershell.exe -NoLogo -NoProfile -Command \"Set-Location -LiteralPath {}; & $env:ComSpec\"",
            quote_powershell(path)
        ),
        _ => {
            let inner = r#"cd -- "$1" && exec "${SHELL:-/bin/sh}" -l"#;
            format!("sh -c {} sh {}", quote_sh(inner), quote_sh(path))
        }
    };
    let mut command = String::from("exec ssh");
    for argument in ssh_args(config)? {
        command.push(' ');
        command.push_str(&quote_sh(&argument));
    }
    command.push_str(" -t ");
    command.push_str(&quote_sh(destination));
    command.push(' ');
    command.push_str(&quote_sh(&remote));
    Ok(command)
}
