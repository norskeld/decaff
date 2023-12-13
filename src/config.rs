use std::fs;
use std::path::{Path, PathBuf};

use kdl::{KdlDocument, KdlEntry, KdlNode};

use crate::app::AppError;
use crate::graph::{DependenciesGraph, Node, Step};

const ARX_CONFIG_NAME: &str = "arx.kdl";

/// Represents a replacement action.
#[derive(Debug)]
pub struct Replacement {
  /// Replacement tag (name).
  ///
  /// ```kdl
  /// replacements {
  ///   TAG "Tag description"
  ///   ^^^
  /// }
  /// ```
  pub tag: String,
  /// Replacement tag description. If not defined, will fallback to `tag`.
  ///
  /// ```kdl
  /// replacements {
  ///   TAG "Tag description"
  ///       ^^^^^^^^^^^^^^^^^
  /// }
  /// ```
  pub description: String,
}

/// Represents an action that can be either an [ActionSuite] *or* an [ActionSingle].
///
/// So actions should be defined either like this:
///
/// ```kdl
/// actions {
///   suite name="suite-one" { ... }
///   suite name="suite-two" { ... }
///   ...
/// }
/// ```
///
/// Or like this:
///
/// ```kdl
/// actions {
///   copy from="path/to/file/or/dir" to="path/to/target"
///   move from="path/to/file/or/dir" to="path/to/target"
///   ...
/// }
/// ```
#[derive(Debug)]
pub enum Action {
  Suite(Vec<ActionSuite>),
  Single(Vec<ActionSingle>),
}

/// A suite of actions that contains a flat list of single actions and may also depend on other
/// suites (hence the **requirements** field).
#[derive(Clone, Debug)]
pub struct ActionSuite {
  /// Suite name.
  pub name: String,
  /// Suite actions to run (synchronously).
  pub actions: Vec<ActionSingle>,
  /// Other actions this suite depends on.
  pub requirements: Vec<String>,
}

impl Node for ActionSuite {
  type Item = String;

  fn dependencies(&self) -> &[Self::Item] {
    &self.requirements[..]
  }

  fn matches(&self, dependency: &Self::Item) -> bool {
    self.name == *dependency
  }
}

/// A single "atomic" action.
#[derive(Clone, Debug)]
pub enum ActionSingle {
  /// Copies a file or directory. Glob-friendly. Overwrites by default.
  Copy {
    from: Option<PathBuf>,
    to: Option<PathBuf>,
    overwrite: bool,
  },
  /// Moves a file or directory. Glob-friendly. Overwrites by default.
  Move {
    from: Option<PathBuf>,
    to: Option<PathBuf>,
    overwrite: bool,
  },
  /// Deletes a file or directory. Glob-friendly.
  Delete { target: Option<PathBuf> },
  /// Runs an arbitrary command in the shell.
  Run { command: Option<String> },
  /// Fallback action for pattern matching ergonomics and reporting purposes.
  Unknown { name: String },
}

/// Checks if arx config exists under the given directory.
pub fn has_arx_config(root: &Path) -> bool {
  // TODO: Allow to override the config name.
  let file = root.join(ARX_CONFIG_NAME);
  let file_exists = file.try_exists();

  file_exists.is_ok()
}

/// Resolves, reads and parses an arx config into a [KdlDocument].
pub fn resolve_arx_config(root: &Path) -> Result<KdlDocument, AppError> {
  let filename = root.join(ARX_CONFIG_NAME);

  let contents = fs::read_to_string(filename)
    .map_err(|_| AppError("Couldn't read the config file.".to_string()))?;

  let document: KdlDocument = contents
    .parse()
    .map_err(|_| AppError("Couldn't parse the config file.".to_string()))?;

  Ok(document)
}

/// Resolves requirements (dependencies) for an [ActionSuite].
pub fn resolve_requirements(suites: &[ActionSuite]) -> (Vec<ActionSuite>, Vec<String>) {
  let graph = DependenciesGraph::from(suites);

  graph.fold((vec![], vec![]), |(mut resolved, mut unresolved), next| {
    match next {
      | Step::Resolved(suite) => resolved.push(suite.clone()),
      | Step::Unresolved(dep) => unresolved.push(dep.clone()),
    }

    (resolved, unresolved)
  })
}

/// Gets actions from a KDL document.
pub fn get_actions(doc: &KdlDocument) -> Option<Action> {
  doc
    .get("actions")
    .and_then(KdlNode::children)
    .map(|children| {
      let nodes = children.nodes();

      if nodes.iter().all(is_suite) {
        let suites = nodes.iter().filter_map(to_action_suite).collect();
        Action::Suite(suites)
      } else {
        let actions = nodes.iter().filter_map(to_action_single).collect();
        Action::Single(actions)
      }
    })
}

/// Gets replacements from a KDL document.
pub fn get_replacements(doc: &KdlDocument) -> Option<Vec<Replacement>> {
  doc
    .get("replacements")
    .and_then(KdlNode::children)
    .map(|children| {
      children
        .nodes()
        .iter()
        .filter_map(to_replacement)
        .collect::<Vec<_>>()
    })
}

// Helpers and mappers.

fn to_replacement(node: &KdlNode) -> Option<Replacement> {
  let tag = node.name().to_string();

  let description = node
    .get(0)
    .and_then(entry_to_string)
    .unwrap_or_else(|| tag.clone());

  Some(Replacement { tag, description })
}

fn to_action_suite(node: &KdlNode) -> Option<ActionSuite> {
  let name = node.get("name").and_then(entry_to_string);

  let requirements = node.get("requires").and_then(entry_to_string).map(|value| {
    value
      .split_ascii_whitespace()
      .map(str::to_string)
      .collect::<Vec<_>>()
  });

  let actions = node.children().map(|children| {
    children
      .nodes()
      .iter()
      .filter_map(to_action_single)
      .collect::<Vec<_>>()
  });

  let suite = (
    name,
    actions.unwrap_or_default(),
    requirements.unwrap_or_default(),
  );

  match suite {
    | (Some(name), actions, requirements) => {
      Some(ActionSuite {
        name,
        actions,
        requirements,
      })
    },
    | _ => None,
  }
}

/// TODO: This probably should be refactored and abstracted away into something separate.
fn to_action_single(node: &KdlNode) -> Option<ActionSingle> {
  let action_kind = node.name().to_string();

  let action = match action_kind.to_ascii_lowercase().as_str() {
    | "copy" => {
      ActionSingle::Copy {
        from: node.get("from").and_then(entry_to_pathbuf),
        to: node.get("to").and_then(entry_to_pathbuf),
        overwrite: node
          .get("overwrite")
          .and_then(entry_to_bool)
          .unwrap_or(true),
      }
    },
    | "move" => {
      ActionSingle::Move {
        from: node.get("from").and_then(entry_to_pathbuf),
        to: node.get("to").and_then(entry_to_pathbuf),
        overwrite: node
          .get("overwrite")
          .and_then(entry_to_bool)
          .unwrap_or(true),
      }
    },
    | "delete" => {
      ActionSingle::Delete {
        target: node.get(0).and_then(entry_to_pathbuf),
      }
    },
    | "run" => {
      ActionSingle::Run {
        command: node.get(0).and_then(entry_to_string),
      }
    },
    | action => {
      ActionSingle::Unknown {
        name: action.to_string(),
      }
    },
  };

  Some(action)
}

fn is_suite(node: &KdlNode) -> bool {
  node.name().value().to_string().eq("suite")
}

fn entry_to_string(entry: &KdlEntry) -> Option<String> {
  entry.value().as_string().map(str::to_string)
}

fn entry_to_bool(entry: &KdlEntry) -> Option<bool> {
  entry.value().as_bool()
}

fn entry_to_pathbuf(entry: &KdlEntry) -> Option<PathBuf> {
  entry.value().as_string().map(PathBuf::from)
}
