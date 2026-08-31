use std::fmt;
use std::io::{self, IsTerminal};

use inquire::error::InquireError;
use inquire::ui::{Attributes, RenderConfig};
use inquire::{Confirm, Select, Text};

use crate::Result;

pub trait Interaction: Send + Sync {
    fn choose(&self, prompt: &str, options: &[String]) -> Result<Option<usize>>;
    fn input(&self, prompt: &str, initial: Option<&str>) -> Result<Option<String>>;
    fn confirm(&self, prompt: &str) -> Result<Option<bool>>;
}

pub struct TerminalInteraction;

const FALLBACK_PAGE_SIZE: usize = 15;
const SELECT_RESERVED_ROWS: u16 = 3;

#[derive(Clone)]
struct Choice {
    index: usize,
    label: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
    }
}

pub fn from_env() -> Box<dyn Interaction> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("TMUX_ATELIER_INTERACTION_FILE") {
        return Box::new(ScriptedInteraction {
            path: path.into(),
            log: std::env::var_os("TMUX_ATELIER_INTERACTION_LOG").map(Into::into),
        });
    }
    Box::new(TerminalInteraction)
}

impl Interaction for TerminalInteraction {
    fn choose(&self, prompt: &str, options: &[String]) -> Result<Option<usize>> {
        let choices = options
            .iter()
            .enumerate()
            .map(|(index, label)| Choice {
                index,
                label: label.clone(),
            })
            .collect();
        Ok(cancelled(
            Select::new(prompt, choices)
                .with_page_size(select_page_size())
                .with_render_config(select_render_config())
                .prompt_skippable(),
        )?
        .map(|selected| selected.index))
    }

    fn input(&self, prompt: &str, initial: Option<&str>) -> Result<Option<String>> {
        if !io::stdin().is_terminal() {
            let mut line = String::new();
            return if io::stdin().read_line(&mut line)? == 0 {
                Ok(None)
            } else {
                Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
            };
        }
        let mut input = Text::new(prompt);
        if let Some(initial) = initial {
            input = input.with_initial_value(initial);
        }
        cancelled(input.prompt_skippable())
    }

    fn confirm(&self, prompt: &str) -> Result<Option<bool>> {
        cancelled(Confirm::new(prompt).with_default(false).prompt_skippable())
    }
}

fn select_page_size() -> usize {
    crossterm::terminal::size()
        .map(|(_, rows)| page_size_for_rows(rows))
        .unwrap_or(FALLBACK_PAGE_SIZE)
}

fn page_size_for_rows(rows: u16) -> usize {
    usize::from(rows.saturating_sub(SELECT_RESERVED_ROWS).max(1))
}

fn select_render_config() -> RenderConfig<'static> {
    let mut config = RenderConfig::default();
    config.highlighted_option_prefix = config
        .highlighted_option_prefix
        .with_content("  ❯")
        .with_attr(Attributes::BOLD);
    config.unhighlighted_option_prefix = config.unhighlighted_option_prefix.with_content("   ");
    let selected = config.selected_option.unwrap_or_default();
    config.selected_option = Some(selected.with_attr(selected.att | Attributes::BOLD));
    config
}

fn cancelled<T>(result: std::result::Result<Option<T>, InquireError>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(value),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(debug_assertions)]
struct ScriptedInteraction {
    path: std::path::PathBuf,
    log: Option<std::path::PathBuf>,
}

#[cfg(debug_assertions)]
impl ScriptedInteraction {
    fn response(&self, kind: &str, prompt: &str, options: &[String]) -> Result<Option<String>> {
        use std::fs::OpenOptions;
        use std::io::{Read, Seek, Write};

        if let Some(path) = &self.log {
            let mut log = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(log, "{kind}\t{prompt}\t{}", options.join("\t"))?;
        }
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.lock()?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let (line, rest) = contents.split_once('\n').unwrap_or((&contents, ""));
        file.rewind()?;
        file.set_len(0)?;
        file.write_all(rest.as_bytes())?;
        file.unlock()?;
        let (actual, value) = line.split_once('\t').unwrap_or((line, ""));
        if actual == "cancel" {
            Ok(None)
        } else if actual == kind {
            Ok(Some(value.to_owned()))
        } else {
            Err(crate::err(format!(
                "expected scripted {kind} response, got: {line}"
            )))
        }
    }
}

#[cfg(debug_assertions)]
impl Interaction for ScriptedInteraction {
    fn choose(&self, prompt: &str, options: &[String]) -> Result<Option<usize>> {
        let Some(value) = self.response("choose", prompt, options)? else {
            return Ok(None);
        };
        options
            .iter()
            .position(|option| option == &value)
            .map(Some)
            .ok_or_else(|| crate::err(format!("scripted choice is unavailable: {value}")))
    }

    fn input(&self, prompt: &str, _initial: Option<&str>) -> Result<Option<String>> {
        self.response("input", prompt, &[])
    }

    fn confirm(&self, prompt: &str) -> Result<Option<bool>> {
        let Some(value) = self.response("confirm", prompt, &[])? else {
            return Ok(None);
        };
        match value.as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(crate::err(format!(
                "invalid scripted confirmation: {value}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_menu_uses_the_expanded_indented_style() {
        let config = select_render_config();
        assert_eq!(config.highlighted_option_prefix.content, "  ❯");
        assert_eq!(config.unhighlighted_option_prefix.content, "   ");
        assert!(
            config
                .selected_option
                .unwrap()
                .att
                .contains(Attributes::BOLD)
        );
        assert_eq!(page_size_for_rows(30), 27);
        assert_eq!(page_size_for_rows(3), 1);
    }
}
