use std::fmt::Display;

use inquire::formatter::StringFormatter;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};
use inquire::{Confirm, Editor, Select, Text};

use crate::actions::{State, Value};
use crate::manifest::prompts;

/// Helper struct holding static methods for convenience.
struct Inquirer;

impl Inquirer {
  /// Returns configured theme.
  pub fn theme<'r>() -> RenderConfig<'r> {
    let default = RenderConfig::default();
    let stylesheet = StyleSheet::default();

    let prompt_prefix = Styled::new("?").with_fg(Color::LightYellow);
    let answered_prefix = Styled::new("✓").with_fg(Color::LightGreen);

    default
      .with_prompt_prefix(prompt_prefix)
      .with_answered_prompt_prefix(answered_prefix)
      .with_answer(stylesheet.with_fg(Color::White))
      .with_default_value(stylesheet.with_fg(Color::DarkGrey))
  }

  /// Returns a formatter that shows `<empty>` if the input is empty.
  pub fn empty_formatter<'s>() -> StringFormatter<'s> {
    &|input| {
      if input.is_empty() {
        "<empty>".to_string()
      } else {
        input.to_string()
      }
    }
  }

  /// Helper method that generates `(name, hint, help)`.
  pub fn messages<S>(name: S, hint: S) -> (String, String, String)
  where
    S: Into<String> + AsRef<str> + Display,
  {
    let name = name.into();
    let hint = format!("{}:", &hint);
    let help = format!("The answer will be mapped to: {}", &name);

    (name, hint, help)
  }
}

impl prompts::Confirm {
  /// Execute the prompt and populate the state.
  pub async fn execute(&self, state: &mut State) -> anyhow::Result<()> {
    let (name, hint, help) = Inquirer::messages(&self.name, &self.hint);

    let mut prompt = Confirm::new(&hint)
      .with_help_message(&help)
      .with_render_config(Inquirer::theme());

    if let Some(default) = self.default {
      prompt = prompt.with_default(default);
    }

    if let Ok(value) = prompt.prompt() {
      state.set(name, Value::Bool(value));
    }

    Ok(())
  }
}

impl prompts::Input {
  /// Execute the prompt and populate the state.
  pub async fn execute(&self, state: &mut State) -> anyhow::Result<()> {
    let (name, hint, help) = Inquirer::messages(&self.name, &self.hint);

    let mut prompt = Text::new(&hint)
      .with_help_message(&help)
      .with_formatter(Inquirer::empty_formatter())
      .with_render_config(Inquirer::theme());

    if let Some(default) = &self.default {
      prompt = prompt.with_default(default);
    }

    if let Ok(value) = prompt.prompt() {
      state.set(name, Value::String(value));
    }

    Ok(())
  }
}

impl prompts::Select {
  /// Execute the prompt and populate the state.
  pub async fn execute(&self, state: &mut State) -> anyhow::Result<()> {
    let (name, hint, help) = Inquirer::messages(&self.name, &self.hint);

    let options = self.options.iter().map(String::to_string).collect();

    let prompt = Select::new(&hint, options)
      .with_help_message(&help)
      .with_render_config(Inquirer::theme());

    if let Ok(value) = prompt.prompt() {
      state.set(name, Value::String(value));
    }

    Ok(())
  }
}

impl prompts::Editor {
  /// Execute the prompt and populate the state.
  pub async fn execute(&self, state: &mut State) -> anyhow::Result<()> {
    let (name, hint, help) = Inquirer::messages(&self.name, &self.hint);

    let mut prompt = Editor::new(&hint)
      .with_help_message(&help)
      .with_render_config(Inquirer::theme());

    if let Some(default) = &self.default {
      prompt = prompt.with_predefined_text(default);
    }

    if let Ok(value) = prompt.prompt() {
      state.set(name, Value::String(value));
    }

    Ok(())
  }
}
