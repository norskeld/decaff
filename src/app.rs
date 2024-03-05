use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::actions::Executor;
use crate::manifest::Manifest;
use crate::repository::{LocalRepository, RemoteRepository};
use crate::unpacker::Unpacker;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
  #[command(subcommand)]
  pub command: BaseCommands,

  /// Delete arx config after scaffolding.
  #[arg(short, long)]
  pub delete: bool,
}

#[derive(Debug, Subcommand)]
pub enum BaseCommands {
  /// Scaffold from a remote repository.
  Remote {
    /// Template repository to use for scaffolding.
    src: String,

    /// Directory to scaffold to.
    path: Option<String>,

    /// Scaffold from a specified ref (branch, tag, or commit).
    #[arg(name = "REF", short = 'r', long = "ref")]
    meta: Option<String>,
  },
  /// Scaffold from a local repository.
  Local {
    /// Template repository to use for scaffolding.
    src: String,

    /// Directory to scaffold to.
    path: Option<String>,

    /// Scaffold from a specified ref (branch, tag, or commit).
    #[arg(name = "REF", short = 'r', long = "ref")]
    meta: Option<String>,
  },
}

#[derive(Debug)]
pub struct App {
  cli: Cli,
}

impl App {
  pub fn new() -> Self {
    Self { cli: Cli::parse() }
  }

  pub async fn run(self) -> anyhow::Result<()> {
    // Load the manifest.
    let manifest = match self.cli.command {
      | BaseCommands::Remote { src, path, meta } => Self::remote(src, path, meta).await?,
      | BaseCommands::Local { src, path, meta } => Self::local(src, path, meta).await?,
    };

    // Create executor and kick off execution.
    let executor = Executor::new(manifest);
    executor.execute().await?;

    Ok(())
  }

  /// Preparation flow for remote repositories.
  async fn remote(
    src: String,
    path: Option<String>,
    meta: Option<String>,
  ) -> anyhow::Result<Manifest> {
    // Parse repository.
    let remote = RemoteRepository::new(src, meta)?;

    let name = path.unwrap_or(remote.repo.clone());
    let destination = PathBuf::from(name);

    // Check if destination already exists before downloading.
    if let Ok(true) = &destination.try_exists() {
      anyhow::bail!("{} already exists", destination.display());
    }

    // Fetch the tarball as bytes (compressed).
    let tarball = remote.fetch().await?;

    // Decompress and unpack the tarball.
    let unpacker = Unpacker::new(tarball);
    unpacker.unpack_to(&destination)?;

    // Now we need to read the manifest (if it is present).
    let mut manifest = Manifest::new(&destination);
    manifest.load()?;

    Ok(manifest)
  }

  /// Preparation flow for local repositories.
  async fn local(
    src: String,
    path: Option<String>,
    meta: Option<String>,
  ) -> anyhow::Result<Manifest> {
    // Create repository.
    let local = LocalRepository::new(src, meta);

    let destination = if let Some(destination) = path {
      PathBuf::from(destination)
    } else {
      local
        .source
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_default()
    };

    // Check if destination already exists before performing local clone.
    if let Ok(true) = &destination.try_exists() {
      anyhow::bail!("{} already exists", destination.display());
    }

    // Copy the directory.
    local.copy(&destination)?;
    local.checkout(&destination)?;

    // Delete inner .git.
    let inner_git = destination.join(".git");

    if let Ok(true) = inner_git.try_exists() {
      println!("Removing {}\n", inner_git.display());
      fs::remove_dir_all(inner_git)?;
    }

    // Now we need to read the manifest (if it is present).
    let mut manifest = Manifest::new(&destination);
    manifest.load()?;

    Ok(manifest)
  }
}

impl Default for App {
  fn default() -> Self {
    Self::new()
  }
}
